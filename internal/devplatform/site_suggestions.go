package devplatform

import (
	"errors"
	"fmt"
	"net/url"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
)

func domainSlug(value string) string {
	var b strings.Builder
	separator := false
	for _, r := range strings.ToLower(value) {
		if r >= 'a' && r <= 'z' || r >= '0' && r <= '9' {
			if separator && b.Len() > 0 {
				b.WriteByte('-')
			}
			b.WriteRune(r)
			separator = false
		} else {
			separator = true
		}
		if b.Len() >= 40 {
			break
		}
	}
	return strings.Trim(b.String(), "-")
}

// Suggest declarations only. This never maps hosts, trusts a CA or starts jobs.
func SuggestProjectSite(s State, id string) (Site, error) {
	p, ok := s.Projects[id]
	if !ok {
		return Site{}, errors.New("project not found")
	}
	base := domainSlug(p.Name)
	if base == "" {
		base = domainSlug(filepath.Base(p.Directory))
	}
	if base == "" {
		base = "project-" + domainSlug(p.ID)
	}
	domains := map[string]bool{}
	ids := map[string]bool{}
	ports := map[int]bool{}
	for _, site := range s.Sites {
		domains[site.Domain] = true
		if !site.Disabled && !site.NeedsReview {
			ports[site.Port] = true
		}
	}
	for _, project := range s.Projects {
		for _, site := range project.Sites {
			domains[site.Domain] = true
			if !site.Disabled && !site.NeedsReview {
				ports[site.Port] = true
			}
		}
	}
	for _, site := range p.Sites {
		ids[site.ID] = true
	}
	name, domain := "web", base+".test"
	for n := 2; ids[name]; n++ {
		name = fmt.Sprintf("web-%d", n)
	}
	for n := 2; domains[domain]; n++ {
		domain = fmt.Sprintf("%s-%d.test", base, n)
	}
	port := 443
	var listeners []int
	for value := range ports {
		listeners = append(listeners, value)
	}
	sort.Ints(listeners)
	if len(listeners) > 0 {
		port = listeners[0]
	}
	upstream := "http://127.0.0.1:3000"
	for _, svc := range p.Services {
		u, err := url.Parse(svc.Health)
		if err == nil && u.Host != "" && (u.Scheme == "tcp" || u.Scheme == "http" || u.Scheme == "https") {
			scheme := u.Scheme
			if scheme == "tcp" {
				scheme = "http"
			}
			candidate := scheme + "://" + u.Host
			if err := (Site{ID: name, Domain: domain, Upstream: candidate, Port: port}).Validate(); err == nil {
				upstream = candidate
				break
			}
		}
		if value, err := strconv.Atoi(svc.Environment["PORT"]); err == nil && value > 0 && value < 65536 {
			upstream = fmt.Sprintf("http://127.0.0.1:%d", value)
			break
		}
	}
	return Site{ID: name, Domain: domain, Upstream: upstream, Port: port}, nil
}
