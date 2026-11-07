package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"
)

// nodeDistOverride points tests at a local server.
var nodeDistOverride string

// nodeDist is where Node.js publishes releases: nodejs.org, or an HTTPS
// mirror named by MSM_NODE_DIST_MIRROR, as FNM_NODE_DIST_MIRROR and
// NVM_NODEJS_ORG_MIRROR do. Checksums come from the same place as the
// archives, so use a mirror you trust.
func nodeDist() (string, error) {
	if nodeDistOverride != "" {
		return nodeDistOverride, nil
	}
	if v := strings.TrimSpace(os.Getenv("MSM_NODE_DIST_MIRROR")); v != "" {
		u, e := url.Parse(v)
		if e != nil || u.Scheme != "https" || u.Host == "" || u.User != nil {
			return "", errors.New("MSM_NODE_DIST_MIRROR must be an HTTPS address")
		}
		return strings.TrimRight(v, "/") + "/", nil
	}
	return "https://nodejs.org/dist/", nil
}

const nodeIndexTTL = time.Hour

// NodeRelease is a line of nodejs.org's release index.
type NodeRelease struct {
	Version  string `json:"version"` // exact, without a leading v
	Date     string `json:"date,omitempty"`
	LTS      string `json:"lts,omitempty"` // code name, empty outside long-term support
	NPM      string `json:"npm,omitempty"`
	V8       string `json:"v8,omitempty"`
	Security bool   `json:"security,omitempty"`
}

type nodeIndexCache struct {
	FetchedAt time.Time     `json:"fetchedAt"`
	Releases  []NodeRelease `json:"releases"`
}

func (m *Manager) nodeIndexPath() string { return filepath.Join(m.Root, "cache", "node-index.json") }

// fetchNodeReleases reads nodejs.org's index and keeps what runs on Apple
// Silicon, newest first.
func fetchNodeReleases(ctx context.Context) ([]NodeRelease, error) {
	var rows []struct {
		Version  string          `json:"version"`
		Date     string          `json:"date"`
		Files    []string        `json:"files"`
		NPM      string          `json:"npm"`
		V8       string          `json:"v8"`
		LTS      json.RawMessage `json:"lts"`
		Security bool            `json:"security"`
	}
	dist, e := nodeDist()
	if e != nil {
		return nil, e
	}
	if e := fetch(ctx, dist+"index.json", &rows); e != nil {
		return nil, fmt.Errorf("nodejs.org release index: %w", e)
	}
	var out []NodeRelease
	for _, r := range rows {
		version := strings.TrimPrefix(r.Version, "v")
		if _, ok := parseSemver(version); !ok {
			continue
		}
		arm := false
		for _, f := range r.Files {
			arm = arm || f == "osx-arm64-tar"
		}
		if !arm {
			continue
		}
		release := NodeRelease{Version: version, Date: r.Date, NPM: r.NPM, V8: r.V8, Security: r.Security}
		var name string
		if json.Unmarshal(r.LTS, &name) == nil {
			release.LTS = name
		}
		out = append(out, release)
	}
	sort.Slice(out, func(i, j int) bool { return versionGreater(out[i].Version, out[j].Version) })
	if len(out) == 0 {
		return nil, errors.New("nodejs.org lists no release for Apple Silicon")
	}
	return out, nil
}

// NodeReleases returns the releases nodejs.org offers for Apple Silicon. The
// index is kept for an hour; with refresh it is read again. When nodejs.org
// cannot be reached the last copy is returned with stale set.
func (m *Manager) NodeReleases(ctx context.Context, refresh bool) (releases []NodeRelease, stale bool, err error) {
	var cached nodeIndexCache
	haveCache := false
	if b, e := os.ReadFile(m.nodeIndexPath()); e == nil && json.Unmarshal(b, &cached) == nil && len(cached.Releases) > 0 {
		haveCache = true
		if !refresh && time.Since(cached.FetchedAt) < nodeIndexTTL {
			return cached.Releases, false, nil
		}
	}
	fresh, e := fetchNodeReleases(ctx)
	if e != nil {
		if haveCache {
			return cached.Releases, true, nil
		}
		return nil, false, e
	}
	if e = writeJSON(m.nodeIndexPath(), nodeIndexCache{FetchedAt: time.Now().UTC(), Releases: fresh}); e != nil {
		m.emit("Release index could not be cached: " + e.Error())
	}
	return fresh, false, nil
}

type semver struct{ major, minor, patch int }

func parseSemver(s string) (semver, bool) {
	parts := strings.Split(strings.TrimPrefix(strings.TrimSpace(s), "v"), ".")
	if len(parts) != 3 {
		return semver{}, false
	}
	var n [3]int
	for i, p := range parts {
		v, e := strconv.Atoi(p)
		if e != nil || v < 0 {
			return semver{}, false
		}
		n[i] = v
	}
	return semver{n[0], n[1], n[2]}, true
}

func (a semver) less(b semver) bool {
	if a.major != b.major {
		return a.major < b.major
	}
	if a.minor != b.minor {
		return a.minor < b.minor
	}
	return a.patch < b.patch
}

func versionGreater(a, b string) bool {
	x, _ := parseSemver(a)
	y, _ := parseSemver(b)
	return y.less(x)
}

// A bound is a half-open interval [lo, hi); an unset hi is open ended.
type nodeBound struct {
	lo, hi semver
	hasHi  bool
}

func (b nodeBound) contains(v semver) bool {
	return !v.less(b.lo) && (!b.hasHi || v.less(b.hi))
}

func (b nodeBound) intersect(o nodeBound) nodeBound {
	if b.lo.less(o.lo) {
		b.lo = o.lo
	}
	if o.hasHi && (!b.hasHi || o.hi.less(b.hi)) {
		b.hi, b.hasHi = o.hi, true
	}
	return b
}

// partialVersion is "22", "22.14", "22.x" or "22.14.0"; missing or wild
// parts are -1.
func parsePartial(s string) ([3]int, bool) {
	s = strings.TrimPrefix(strings.TrimSpace(s), "v")
	parts := strings.Split(s, ".")
	if s == "" || len(parts) > 3 {
		return [3]int{}, false
	}
	n := [3]int{-1, -1, -1}
	wild := false
	for i, p := range parts {
		if p == "x" || p == "X" || p == "*" {
			wild = true
			continue
		}
		if wild {
			return n, false // 22.x.1 names nothing
		}
		v, e := strconv.Atoi(p)
		if e != nil || v < 0 {
			return n, false
		}
		n[i] = v
	}
	return n, true
}

func floor(n [3]int) semver {
	for i := range n {
		n[i] = max(n[i], 0)
	}
	return semver{n[0], n[1], n[2]}
}

// parseComparator turns one comparator of a semver range into a bound.
func parseComparator(token string) (nodeBound, error) {
	op := ""
	for _, candidate := range []string{">=", "<=", ">", "<", "=", "^", "~"} {
		if strings.HasPrefix(token, candidate) {
			op, token = candidate, strings.TrimSpace(strings.TrimPrefix(token, candidate))
			break
		}
	}
	n, ok := parsePartial(token)
	if !ok {
		return nodeBound{}, fmt.Errorf("%q is not a Node.js version or range", token)
	}
	lo := floor(n)
	next := func() semver { // the first version after everything the partial names
		switch {
		case n[0] < 0:
			return semver{}
		case n[1] < 0:
			return semver{n[0] + 1, 0, 0}
		case n[2] < 0:
			return semver{n[0], n[1] + 1, 0}
		}
		return semver{n[0], n[1], n[2] + 1}
	}
	switch op {
	case "", "=":
		if n[0] < 0 {
			return nodeBound{}, nil
		}
		return nodeBound{lo: lo, hi: next(), hasHi: true}, nil
	case "^":
		switch {
		case n[0] < 0:
			return nodeBound{}, nil
		case n[0] > 0 || n[1] < 0:
			return nodeBound{lo: lo, hi: semver{n[0] + 1, 0, 0}, hasHi: true}, nil
		case n[1] > 0 || n[2] < 0:
			return nodeBound{lo: lo, hi: semver{0, n[1] + 1, 0}, hasHi: true}, nil
		}
		return nodeBound{lo: lo, hi: semver{0, 0, n[2] + 1}, hasHi: true}, nil
	case "~":
		if n[0] < 0 {
			return nodeBound{}, nil
		}
		if n[1] < 0 {
			return nodeBound{lo: lo, hi: semver{n[0] + 1, 0, 0}, hasHi: true}, nil
		}
		return nodeBound{lo: lo, hi: semver{n[0], n[1] + 1, 0}, hasHi: true}, nil
	case ">=":
		return nodeBound{lo: lo}, nil
	case ">":
		return nodeBound{lo: next()}, nil
	case "<":
		return nodeBound{hi: lo, hasHi: true}, nil
	case "<=":
		return nodeBound{hi: next(), hasHi: true}, nil
	}
	return nodeBound{}, fmt.Errorf("unsupported comparator %q", op)
}

// parseRange reads a semver range as nvm, fnm and package.json "engines"
// write it: 22, 22.14, 22.14.0, 22.x, ^20.1, ~20.1.2, >=18 <23, 18 || 20 and
// 18.0.0 - 20.5.0. The result is a union of bounds.
func parseRange(spec string) ([]nodeBound, error) {
	var out []nodeBound
	for _, set := range strings.Split(spec, "||") {
		fields := strings.Fields(set)
		if len(fields) == 0 {
			out = append(out, nodeBound{})
			continue
		}
		// "a - b" is the same as ">=a <=b".
		var tokens []string
		for i := 0; i < len(fields); i++ {
			if i+2 < len(fields)+0 && fields[i+1] == "-" {
				tokens = append(tokens, ">="+fields[i], "<="+fields[i+2])
				i += 2
				continue
			}
			// ">= 18" is ">=18".
			if (fields[i] == ">=" || fields[i] == ">" || fields[i] == "<" || fields[i] == "<=" || fields[i] == "^" || fields[i] == "~" || fields[i] == "=") && i+1 < len(fields) {
				fields[i+1] = fields[i] + fields[i+1]
				continue
			}
			tokens = append(tokens, fields[i])
		}
		bound := nodeBound{}
		for _, token := range tokens {
			b, e := parseComparator(token)
			if e != nil {
				return nil, e
			}
			bound = bound.intersect(b)
		}
		out = append(out, bound)
	}
	return out, nil
}

// nodeMatcher reads what a user, an .nvmrc or an "engines" field says:
// latest | node | current, lts | lts/* | lts/<name>, a version or a range.
func nodeMatcher(spec string) (func(NodeRelease) bool, error) {
	s := strings.ToLower(strings.TrimSpace(spec))
	switch {
	case s == "":
		return nil, errors.New("name a Node.js version, for example 22, 22.14.0, lts or latest")
	case s == "latest" || s == "node" || s == "current" || s == "stable":
		return func(NodeRelease) bool { return true }, nil
	case s == "lts" || s == "lts/*" || s == "lts/latest":
		return func(r NodeRelease) bool { return r.LTS != "" }, nil
	case strings.HasPrefix(s, "lts/"):
		name := strings.TrimPrefix(s, "lts/")
		return func(r NodeRelease) bool { return strings.ToLower(r.LTS) == name }, nil
	}
	bounds, e := parseRange(s)
	if e != nil {
		return nil, e
	}
	return func(r NodeRelease) bool {
		v, ok := parseSemver(r.Version)
		if !ok {
			return false
		}
		for _, b := range bounds {
			if b.contains(v) {
				return true
			}
		}
		return false
	}, nil
}

// PickNode returns the newest release among candidates that spec names.
func PickNode(spec string, candidates []NodeRelease) (NodeRelease, bool, error) {
	match, e := nodeMatcher(spec)
	if e != nil {
		return NodeRelease{}, false, e
	}
	var best NodeRelease
	found := false
	for _, r := range candidates {
		if match(r) && (!found || versionGreater(r.Version, best.Version)) {
			best, found = r, true
		}
	}
	return best, found, nil
}

// ResolveNodeSpec turns a name such as 22, lts/iron or latest into the exact
// release nodejs.org offers.
func (m *Manager) ResolveNodeSpec(ctx context.Context, spec string) (NodeRelease, error) {
	if _, e := nodeMatcher(spec); e != nil {
		return NodeRelease{}, e
	}
	releases, _, e := m.NodeReleases(ctx, false)
	if e != nil {
		return NodeRelease{}, e
	}
	r, ok, _ := PickNode(spec, releases)
	if !ok {
		// A release newer than the cached index is possible.
		if fresh, _, e := m.NodeReleases(ctx, true); e == nil {
			r, ok, _ = PickNode(spec, fresh)
		}
	}
	if !ok {
		return NodeRelease{}, fmt.Errorf("nodejs.org has no Node.js release for %q on Apple Silicon", strings.TrimSpace(spec))
	}
	return r, nil
}
