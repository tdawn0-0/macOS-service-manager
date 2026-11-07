package devplatform

import (
	"errors"
	"fmt"
	"net"
	"net/url"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

// Schema 2 is the shareable layer. Materialized Project values stay in private
// application storage. Ports are references, not machine-specific numbers.
type sharedPort struct {
	ID       string `json:"id"`
	Kind     string `json:"kind"`
	Instance string `json:"instance,omitempty"`
}
type sharedService struct {
	Service
	EnvironmentPorts map[string]string `json:"environmentPorts,omitempty"`
}
type sharedSite struct {
	Site
	Listener string `json:"listener"`
}
type sharedProject struct {
	LockSHA256    string          `json:"lockSHA256,omitempty"`
	SchemaVersion int             `json:"schemaVersion"`
	ID            string          `json:"id"`
	Name          string          `json:"name"`
	Directory     string          `json:"directory"`
	Services      []sharedService `json:"services"`
	Instances     []string        `json:"instances,omitempty"`
	Sites         []sharedSite    `json:"sites,omitempty"`
	Ports         []sharedPort    `json:"ports,omitempty"`
}
type sharePorts struct {
	values       map[int]string
	listeners    map[int]string
	declarations []sharedPort
}

func (r *sharePorts) add(value int, kind, instance string) string {
	if value < 1 || value > 65535 {
		return ""
	}
	target := r.values
	if kind == "https" {
		target = r.listeners
	}
	if id := target[value]; id != "" {
		return id
	}
	id := fmt.Sprintf("port-%d", len(r.declarations)+1)
	target[value] = id
	r.declarations = append(r.declarations, sharedPort{id, kind, instance})
	return id
}
func portReference(id string) string { return "{{port:" + id + "}}" }

var portReferenceRE = regexp.MustCompile(`\{\{port:([a-zA-Z0-9._-]+)\}\}`)

func addressPort(address string) int {
	u, err := url.Parse(address)
	if err == nil && u.Host != "" {
		n, _ := strconv.Atoi(u.Port())
		if n == 0 && u.Port() == "" {
			if u.Scheme == "http" {
				return 80
			}
			if u.Scheme == "https" {
				return 443
			}
		}
		return n
	}
	_, port, err := net.SplitHostPort(address)
	if err == nil {
		n, _ := strconv.Atoi(port)
		return n
	}
	return 0
}
func sensitiveFlag(flag string) bool {
	flag = strings.ToLower(strings.SplitN(flag, "=", 2)[0])
	for _, word := range []string{"password", "passwd", "token", "secret", "api-key", "apikey", "authorization", "credential"} {
		if strings.Contains(flag, word) {
			return true
		}
	}
	return flag == "-a" // Redis authentication; conservative for other commands.
}
func portFlag(flag, program string) bool {
	switch strings.ToLower(flag) {
	case "--port", "--listen-port", "--http-port", "--https-port", "--tcp-port":
		return true
	case "-p":
		return program != "mysql" && program != "mysqldump" && program != "mysqladmin" && program != "node"
	}
	return false
}
func argumentPort(arg, previous, program string) int {
	if sensitiveFlag(previous) {
		return 0
	}
	if n := addressPort(arg); n > 0 {
		return n
	}
	if key, value, ok := strings.Cut(arg, "="); ok && portFlag(key, program) {
		n, _ := strconv.Atoi(value)
		return n
	}
	if portFlag(previous, program) {
		n, _ := strconv.Atoi(arg)
		return n
	}
	return 0
}
func rewritePort(address string, ports map[int]string) (string, bool) {
	if n, err := strconv.Atoi(address); err == nil {
		if id := ports[n]; id != "" {
			return portReference(id), true
		}
	}
	if key, value, ok := strings.Cut(address, "="); ok {
		if n, err := strconv.Atoi(value); err == nil {
			if id := ports[n]; id != "" {
				return key + "=" + portReference(id), true
			}
		}
	}
	u, err := url.Parse(address)
	if err == nil && u.Host != "" {
		if u.User != nil || u.RawQuery != "" || u.Fragment != "" {
			return "", false
		}
		n := addressPort(address)
		if id := ports[n]; id != "" {
			// Construct the host text without url.URL escaping template braces.
			return u.Scheme + "://" + net.JoinHostPort(u.Hostname(), portReference(id)) + u.EscapedPath(), true
		}
	}
	if host, port, err := net.SplitHostPort(address); err == nil {
		n, _ := strconv.Atoi(port)
		if id := ports[n]; id != "" {
			return net.JoinHostPort(host, portReference(id)), true
		}
	}
	return "", false
}
func portableLiteral(arg string) bool {
	if arg == "" || filepath.IsAbs(arg) || strings.ContainsAny(arg, "\x00\r\n\t ;|&$(){}\"'`") {
		return false
	}
	if strings.HasPrefix(arg, "-") && !strings.Contains(arg, "=") {
		return regexp.MustCompile(`^--?[a-zA-Z][a-zA-Z-]*$`).MatchString(arg)
	}
	switch arg {
	case "run", "dev", "start", "serve", "ci", "install", "build", "http.server", "localhost", "127.0.0.1", "::1", "0.0.0.0", ".":
		return true
	}
	if key, value, ok := strings.Cut(arg, "="); ok && strings.HasPrefix(key, "-") && !sensitiveFlag(key) {
		return portableLiteral(value)
	}
	if safeRelative(arg) {
		switch strings.ToLower(filepath.Ext(arg)) {
		case ".js", ".mjs", ".cjs", ".ts", ".tsx", ".go", ".php", ".py", ".rb", ".sh", ".json", ".yaml", ".yml", ".toml", ".html":
			return true
		}
	}
	return false
}

func makeSharedProject(p Project, s *State, lock *EnvironmentLock) (sharedProject, error) {
	d := sharedProject{SchemaVersion: 2, ID: p.ID, Name: p.Name, Directory: ".", Instances: append([]string{}, p.Instances...)}
	ports := sharePorts{values: map[int]string{}, listeners: map[int]string{}}
	for _, ref := range p.Instances {
		i, ok := s.Instances[ref]
		if !ok {
			return d, fmt.Errorf("instance %s is missing", ref)
		}
		ports.add(i.Port, "database", ref)
		i.Port = 0
		lock.Instances[ref] = i
	}
	for _, svc := range p.Services {
		if n := addressPort(svc.Health); n > 0 {
			ports.add(n, "application", "")
		}
		for n, arg := range svc.Command {
			previous := ""
			if n > 0 {
				previous = svc.Command[n-1]
			}
			if port := argumentPort(arg, previous, svc.Command[0]); port > 0 {
				ports.add(port, "application", "")
			}
		}
	}
	servicePorts := map[int]bool{}
	for port := range ports.values {
		servicePorts[port] = true
	}
	for _, svc := range p.Services {
		for key, value := range svc.Environment {
			if key == "PORT" || strings.HasSuffix(key, "_PORT") {
				if port, err := strconv.Atoi(value); err == nil && !sensitiveFlag(key) {
					servicePorts[port] = true
					ports.add(port, "application", "")
				}
			}
		}
	}
	for _, site := range p.Sites {
		if n := addressPort(site.Upstream); n > 0 {
			ports.add(n, "application", "")
		}
		ports.add(site.Port, "https", "")
	}
	for _, original := range p.Services {
		svc := original
		svc.Command = append([]string{}, original.Command...)
		svc.Environment = nil
		svc.LocalArguments = nil
		svc.RequiredEnvironment = append([]string{}, original.RequiredEnvironment...)
		ss := sharedService{Service: svc, EnvironmentPorts: map[string]string{}}
		for _, key := range diagnosticKeys(original.Environment) {
			if !containsString(ss.RequiredEnvironment, key) {
				ss.RequiredEnvironment = append(ss.RequiredEnvironment, key)
			}
			if n, err := strconv.Atoi(original.Environment[key]); err == nil && !sensitiveFlag(key) && (key == "PORT" || strings.HasSuffix(key, "_PORT")) {
				if id := ports.values[n]; id != "" {
					ss.EnvironmentPorts[key] = id
				}
			}
		}
		sort.Strings(ss.RequiredEnvironment)
		if filepath.IsAbs(svc.EnvFile) {
			ss.EnvFile = ""
			ss.NeedsEnvironmentFile = true
		}
		if svc.Health != "" && svc.Health != "process" {
			if value, ok := rewritePort(svc.Health, ports.values); ok {
				ss.Health = value
			} else {
				u, _ := url.Parse(svc.Health)
				if u != nil && (u.User != nil || u.RawQuery != "" || u.Fragment != "") {
					ss.Health = ""
					ss.NeedsHealthReview = true
				}
			}
		}
		local := map[int]LocalArgument{}
		for _, arg := range original.LocalArguments {
			local[arg.Index] = arg
		}
		for n, arg := range original.Command {
			kind, label, prefix := "text", fmt.Sprintf("Argument %d on this Mac", n+1), ""
			previous := ""
			if n > 0 {
				previous = original.Command[n-1]
			}
			known, wasLocal := local[n]
			if wasLocal {
				kind, label = known.Kind, known.Label
				prefix = known.Prefix
			}
			secret := sensitiveFlag(previous) || (strings.HasPrefix(arg, "-") && strings.Contains(arg, "=") && sensitiveFlag(arg)) || (strings.HasPrefix(previous, "-p") && strings.HasPrefix(original.Command[0], "mysql"))
			if (strings.HasPrefix(original.Command[0], "mysql") && strings.HasPrefix(arg, "-p") || strings.HasPrefix(original.Command[0], "redis") && strings.HasPrefix(arg, "-a")) && len(arg) > 2 {
				secret = true
				prefix = arg[:2]
			}
			if u, e := url.Parse(arg); e == nil && u.User != nil {
				secret = true
			}
			if secret {
				kind = "secret"
				label = fmt.Sprintf("Private value for argument %d", n+1)
			}
			if !wasLocal && !secret {
				if n == 0 && svc.Package != "" && !filepath.IsAbs(arg) {
					continue
				}
				if value, ok := rewritePort(arg, ports.values); ok {
					ss.Command[n] = value
					continue
				}
				if n > 0 && portableLiteral(arg) {
					continue
				}
			}
			if n == 0 && svc.Package == "" {
				kind = "executable"
				label = "Executable path on this Mac"
			} else if !secret && (filepath.IsAbs(arg) || strings.Contains(arg, "=/")) {
				kind = "path"
				label = fmt.Sprintf("Local path for argument %d", n+1)
			}
			if key, _, ok := strings.Cut(arg, "="); ok && strings.HasPrefix(key, "-") && !strings.ContainsAny(key, " /\\") {
				prefix = key + "="
			}
			ss.LocalArguments = append(ss.LocalArguments, LocalArgument{Index: n, Kind: kind, Label: label, Prefix: prefix})
			ss.Command[n] = LocalArgumentPlaceholder(n)
		}
		d.Services = append(d.Services, ss)
	}
	for _, original := range p.Sites {
		site := original
		site.Port = 0
		if !servicePorts[addressPort(original.Upstream)] {
			site.NeedsReview = true
		}
		u, err := url.Parse(original.Upstream)
		if err != nil {
			return d, err
		}
		if u.User != nil || u.RawQuery != "" || u.Fragment != "" {
			u.User = nil
			u.RawQuery = ""
			u.Fragment = ""
			u.Path = ""
			site.NeedsReview = true
		}
		value, ok := rewritePort(u.String(), ports.values)
		if !ok {
			return d, errors.New("site upstream needs an explicit port before sharing")
		}
		site.Upstream = value
		d.Sites = append(d.Sites, sharedSite{Site: site, Listener: ports.listeners[original.Port]})
	}
	d.Ports = ports.declarations
	return d, nil
}
func containsString(values []string, want string) bool {
	for _, s := range values {
		if s == want {
			return true
		}
	}
	return false
}

func replacePortReferences(value string, ports map[string]int) (string, error) {
	var failure error
	value = portReferenceRE.ReplaceAllStringFunc(value, func(token string) string {
		id := portReferenceRE.FindStringSubmatch(token)[1]
		port, ok := ports[id]
		if !ok {
			failure = fmt.Errorf("unknown shared port reference %s", id)
			return token
		}
		return strconv.Itoa(port)
	})
	if strings.Contains(value, "{{port:") {
		failure = errors.New("malformed shared port reference")
	}
	return value, failure
}

func (m *Manager) materializeSharedProject(d sharedProject, dir, id string) (Project, error) {
	p := Project{SchemaVersion: Schema, ID: id, Name: d.Name, Directory: dir, Instances: append([]string{}, d.Instances...), Services: []Service{}}
	if d.SchemaVersion != 2 {
		return p, errors.New("unsupported shared environment schema")
	}
	for _, svc := range d.Services {
		p.Services = append(p.Services, svc.Service)
	}
	if err := m.importEnvironmentLock(&p); err != nil {
		return p, err
	}
	state, err := m.Read()
	if err != nil {
		return p, err
	}
	reserved := map[int]bool{}
	for _, i := range state.Instances {
		reserved[i.Port] = true
	}
	for _, project := range state.Projects {
		for _, svc := range project.Services {
			reserved[addressPort(svc.Health)] = true
		}
	}
	for _, site := range allSites(&state) {
		reserved[addressPort(site.Upstream)] = true
		reserved[site.Port] = true
	}
	dbPorts := map[string]int{}
	if len(d.Instances) > 0 {
		lock, err := readEnvironmentLock(m.projectLockPath(id))
		if err != nil {
			return p, err
		}
		for n, ref := range d.Instances {
			port := lock.Instances[p.Instances[n]].Port
			dbPorts[ref] = port
			reserved[port] = true
		}
	}
	values := map[string]int{}
	var listeners []int
	seenListeners := map[int]bool{}
	for _, site := range allSites(&state) {
		if !seenListeners[site.Port] {
			listeners = append(listeners, site.Port)
			seenListeners[site.Port] = true
		}
	}
	sort.Ints(listeners)
	if len(listeners) == 0 {
		listeners = []int{443}
	}
	listenerIndex := 0
	for _, decl := range d.Ports {
		if err := validID(decl.ID); err != nil {
			return p, err
		}
		if _, exists := values[decl.ID]; exists {
			return p, errors.New("duplicate shared port reference")
		}
		switch decl.Kind {
		case "database":
			port, ok := dbPorts[decl.Instance]
			if !ok {
				return p, errors.New("shared port references an unknown database")
			}
			values[decl.ID] = port
		case "https":
			if listenerIndex < len(listeners) {
				values[decl.ID] = listeners[listenerIndex]
			} else {
				port, err := freeInstancePort(0, reserved)
				if err != nil {
					return p, err
				}
				values[decl.ID] = port
				reserved[port] = true
			}
			listenerIndex++
		case "application":
			port, err := freeInstancePort(0, reserved)
			if err != nil {
				return p, err
			}
			reserved[port] = true
			values[decl.ID] = port
		default:
			return p, errors.New("unsupported shared port kind")
		}
	}
	for n := range p.Services {
		svc := &p.Services[n]
		for j, arg := range svc.Command {
			value, err := replacePortReferences(arg, values)
			if err != nil {
				return p, err
			}
			svc.Command[j] = value
		}
		svc.Health, err = replacePortReferences(svc.Health, values)
		if err != nil {
			return p, err
		}
		// Ignore values injected into shared local-only definitions.
		for j := range svc.LocalArguments {
			a := &svc.LocalArguments[j]
			a.Value = ""
			if a.Index < 0 || a.Index >= len(svc.Command) {
				return p, errors.New("invalid shared local argument position")
			}
			svc.Command[a.Index] = LocalArgumentPlaceholder(a.Index)
		}
		svc.Environment = nil
		if filepath.IsAbs(svc.EnvFile) {
			svc.EnvFile = ""
			svc.NeedsEnvironmentFile = true
		}
		for key, ref := range d.Services[n].EnvironmentPorts {
			port, ok := values[ref]
			if !ok {
				return p, errors.New("environment references an unknown port")
			}
			if svc.Environment == nil {
				svc.Environment = map[string]string{}
			}
			svc.Environment[key] = strconv.Itoa(port)
		}
	}
	usedDomains := map[string]bool{}
	domainMapping := map[string]string{}
	for _, site := range allSites(&state) {
		usedDomains[site.Domain] = true
	}
	for _, shared := range d.Sites {
		site := shared.Site
		site.Port = values[shared.Listener]
		if site.Port == 0 {
			return p, errors.New("site references an unknown HTTPS listener")
		}
		site.Upstream, err = replacePortReferences(site.Upstream, values)
		if err != nil {
			return p, err
		}
		originalDomain := site.Domain
		if mapped := domainMapping[originalDomain]; mapped != "" {
			site.Domain = mapped
		} else if usedDomains[site.Domain] {
			base := strings.TrimSuffix(site.Domain, ".test")
			if len(base) > 45 {
				base = base[:45]
			}
			site.Domain = strings.TrimRight(base, "-.") + "-" + id[:6] + ".test"
		}
		domainMapping[originalDomain] = site.Domain
		usedDomains[site.Domain] = true
		p.Sites = append(p.Sites, site)
	}
	return p, p.Validate()
}
