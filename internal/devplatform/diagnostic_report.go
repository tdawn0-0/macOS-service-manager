package devplatform

import (
	"context"
	"crypto/tls"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/url"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

// DiagnosticCheck deliberately contains no command output, environment, request
// headers, log data or raw errors. Resource is the only identifying field.
type DiagnosticCheck struct {
	Layer    string `json:"layer"`
	Resource string `json:"resource"`
	Status   string `json:"status"` // OK, Warning, Failed, Stopped, Disabled
	Summary  string `json:"summary"`
	Remedy   string `json:"remedy,omitempty"`
}

type DiagnosticReport struct {
	SchemaVersion int               `json:"schemaVersion"`
	GeneratedAt   time.Time         `json:"generatedAt"`
	Redacted      bool              `json:"redacted"`
	Checks        []DiagnosticCheck `json:"checks"`
}

func (r DiagnosticReport) Text() string {
	var lines []string
	for _, c := range r.Checks {
		line := fmt.Sprintf("[%s] %s / %s: %s", c.Status, c.Layer, c.Resource, c.Summary)
		if c.Remedy != "" {
			line += " — " + c.Remedy
		}
		lines = append(lines, line)
	}
	return strings.Join(lines, "\n")
}

// RedactedJSON replaces local resource names and domains with report-local aliases.
// Fixed summaries preserve the failing layer and repair instruction. No state or
// filesystem traversal is part of export; even private keys are never read.
func (r DiagnosticReport) RedactedJSON() ([]byte, error) {
	r.Checks = append([]DiagnosticCheck{}, r.Checks...)
	aliases := map[string]string{}
	for i := range r.Checks {
		resource := r.Checks[i].Resource
		if aliases[resource] == "" {
			aliases[resource] = fmt.Sprintf("resource-%d", len(aliases)+1)
		}
		r.Checks[i].Resource = aliases[resource]
	}
	r.Redacted = true
	b, err := json.MarshalIndent(r, "", "  ")
	return append(b, '\n'), err
}

func (m *Manager) ExportDiagnostics(ctx context.Context, r DiagnosticReport) (string, error) {
	if err := ctx.Err(); err != nil {
		return "", err
	}
	b, err := r.RedactedJSON()
	if err != nil {
		return "", err
	}
	// A new private file for each export; never overwrite a previous report.
	dir := filepath.Join(m.Root, "reports")
	if err = os.MkdirAll(dir, 0700); err != nil {
		return "", err
	}
	f, err := os.CreateTemp(dir, "diagnostics-*.json")
	if err != nil {
		return "", err
	}
	path := f.Name()
	defer func() {
		if err != nil {
			os.Remove(path)
		}
	}()
	if err = f.Chmod(0600); err == nil {
		_, err = f.Write(b)
	}
	if err == nil {
		err = f.Sync()
	}
	closeErr := f.Close()
	if err == nil {
		err = closeErr
	}
	return path, err
}

type diagnosticProbes struct {
	lookup func(context.Context, string) ([]string, error)
	dial   func(context.Context, string, string) (net.Conn, error)
	tls    func(context.Context, string, string) error
}

func (m *Manager) Diagnose(ctx context.Context) (DiagnosticReport, error) {
	s, err := m.Read()
	if err != nil {
		return DiagnosticReport{}, err
	}
	dialer := &net.Dialer{Timeout: time.Second}
	return m.diagnose(ctx, s, diagnosticProbes{
		lookup: net.DefaultResolver.LookupHost,
		dial:   dialer.DialContext,
		tls: func(ctx context.Context, address, name string) error {
			// Use the OS roots. Trusting an isolated CA here would hide a real
			// browser trust problem. Neither TLS verification nor SNI is bypassed.
			conn, err := (&tls.Dialer{NetDialer: dialer, Config: &tls.Config{ServerName: name, MinVersion: tls.VersionTLS12}}).DialContext(ctx, "tcp", address)
			if conn != nil {
				conn.Close()
			}
			return err
		},
	})
}

func diagnosticKeys[T any](m map[string]T) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func (m *Manager) diagnose(ctx context.Context, s State, p diagnosticProbes) (DiagnosticReport, error) {
	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	r := DiagnosticReport{SchemaVersion: 1, GeneratedAt: time.Now().UTC(), Checks: []DiagnosticCheck{}}
	add := func(layer, resource, status, summary, remedy string) {
		r.Checks = append(r.Checks, DiagnosticCheck{layer, resource, status, summary, remedy})
	}
	for _, id := range diagnosticKeys(s.Packages) {
		result, err := m.VerifyPackage(ctx, id)
		switch {
		case err != nil:
			add("Package", id, "Warning", "Installed-file verification did not complete.", "Retry Check files in Software before using this version.")
		case result.Status == "Verified":
			add("Package", id, "OK", "Installed contents, permissions and symlink targets match the recorded installation index.", "")
		case result.Status == "Not recorded":
			add("Package", id, "Warning", "No trusted installation index is recorded for this version.", "Stop its users, then use Repair in Software to restore from the verified archive.")
		default:
			add("Package", id, "Failed", "Installed files or the installation index need repair.", "Stop its users, then use Repair in Software to restore the same exact version.")
		}
	}
	for _, id := range diagnosticKeys(s.Projects) {
		project := s.Projects[id]
		if err := projectFolderError(project.Directory); err != nil {
			add("Project folder", "Project "+project.Name, "Failed", err.Error(), "Stop the project, choose its current working folder in Projects, save and apply the folder preview.")
		} else {
			add("Project folder", "Project "+project.Name, "OK", "Working folder is available.", "")
		}
		for _, svc := range project.Services {
			if err := ctx.Err(); err != nil {
				return r, err
			}
			resource := "Project " + project.Name + " / " + svc.ID
			state := m.observeService(ctx, "p-"+id+"-"+svc.ID, svc)
			switch state.Status {
			case "Stopped":
				add("Process", resource, "Stopped", "Service is not registered in launchd.", "Start the project when it is needed.")
			case "Failed":
				add("Process", resource, "Failed", "Service is registered but its process is not running. "+state.Message, "Read the service logs, correct the command or environment, then restart the project.")
			case "Restarting":
				add("Process", resource, "Warning", state.Message, "Read service logs if repeated exits continue; stop the project to disable restarts.")
			case "Running":
				add("Readiness", resource, "Warning", "Process is running; readiness is not configured.", "Add a process, TCP or HTTP readiness check in Projects → Services.")
			case "Not ready":
				add("Readiness", resource, "Failed", state.Message, "Check the readiness address, application logs and listening port.")
			case "Healthy":
				add("Readiness", resource, "OK", state.Message, "")
			default:
				add("Process", resource, "Warning", "Process observation did not complete.", "Retry diagnostics before changing the service configuration.")
			}
		}
	}
	for _, id := range diagnosticKeys(s.Instances) {
		instance := s.Instances[id]
		resource := "Database " + id
		if !m.loaded(ctx, "db-"+id) {
			add("Database", resource, "Stopped", "Database service is not registered in launchd.", "Start the instance in Databases when it is needed.")
			continue
		}
		conn, err := p.dial(ctx, "tcp", net.JoinHostPort("127.0.0.1", fmt.Sprint(instance.Port)))
		if conn != nil {
			conn.Close()
		}
		if err != nil {
			add("Database", resource, "Failed", "Database is registered but its loopback port is not reachable.", "Read the instance logs and check for a conflicting port.")
		} else {
			add("Database", resource, "OK", "Database loopback port is reachable; authentication was not tested.", "")
		}
	}
	authority, authorityErr := m.readActiveAuthority()
	if authorityErr != nil || authority.info.Status == "Not created" {
		add("CA", "Local CA", "Warning", "Local CA has not been created or its storage could not be read.", "Open HTTPS → Local certificates to inspect or prepare the authority.")
	} else {
		info := authority.info
		if info.ID != "" {
			if err := m.authorityAliasesValid(); err != nil {
				info.Status = "Invalid"
				info.Usable = false
				info.Detail = err.Error()
			}
		}
		resource := "Local CA"
		if info.Fingerprint != "" {
			resource += " / SHA-256 " + info.Fingerprint + " / expires " + info.NotAfter.Format("2006-01-02")
		}
		status := "Failed"
		if info.Usable {
			status = "OK"
			if info.Status == "Expiring soon" {
				status = "Warning"
			}
		}
		remedy := ""
		if status != "OK" {
			remedy = "Open HTTPS → Local certificates. Prepare and trust a replacement, stop the proxy, activate the reviewed CA, then apply proxy."
		}
		add("CA", resource, status, info.Detail, remedy)
		if authority.cert != nil {
			trust := m.verifyAuthorityTrust(ctx, info.CertificatePath)
			switch trust {
			case "Trusted":
				add("Trust", resource, "OK", "Local CA is trusted by macOS.", "")
			case "Unknown":
				add("Trust", resource, "Warning", "macOS trust observation did not complete.", "Refresh certificate status before changing trust.")
			default:
				add("Trust", resource, "Failed", "macOS did not verify local CA trust.", "Use Local certificates and complete macOS authorization when requested.")
			}
		}
	}
	var sites []Site
	for _, site := range s.Sites {
		sites = append(sites, site)
	}
	for _, project := range s.Projects {
		sites = append(sites, project.Sites...)
	}
	sort.Slice(sites, func(i, j int) bool { return sites[i].Domain < sites[j].Domain })
	for _, site := range sites {
		if err := ctx.Err(); err != nil {
			return r, err
		}
		if site.NeedsReview && !site.Disabled {
			add("Site", site.Domain, "Warning", "Imported upstream needs local review and was not probed.", "Review the upstream in HTTPS, then save the site.")
			continue
		}
		if site.Disabled {
			add("Site", site.Domain, "Disabled", "Site is disabled and was not probed.", "")
			continue
		}
		// macOS can take roughly five seconds to return a local hosts result
		// while it finishes another address-family query. Do not turn that
		// normal resolver delay into a false configuration failure.
		lookupCtx, done := context.WithTimeout(ctx, 8*time.Second)
		addresses, err := p.lookup(lookupCtx, site.Domain)
		done()
		correct := err == nil && len(addresses) > 0
		for _, address := range addresses {
			if !net.ParseIP(address).Equal(net.ParseIP("127.0.0.1")) {
				correct = false
			}
		}
		var dnsError net.Error
		timedOut := errors.Is(err, context.DeadlineExceeded) || (errors.As(err, &dnsError) && dnsError.Timeout())
		if timedOut {
			add("DNS", site.Domain, "Warning", "Name resolution timed out; the domain mapping was not verified.", "Retry diagnostics and inspect the macOS resolver if the timeout repeats.")
		} else if correct {
			add("DNS", site.Domain, "OK", "Domain resolves only to the proxy's IPv4 loopback address.", "")
		} else {
			add("DNS", site.Domain, "Failed", "Domain is unresolved or includes an address other than 127.0.0.1.", "Use Configure local domains; remove conflicting DNS or hosts entries, then retry.")
		}
		u, err := url.Parse(site.Upstream)
		if err != nil || u.Hostname() == "" || (u.Scheme != "http" && u.Scheme != "https") {
			add("Upstream", site.Domain, "Failed", "Upstream address is invalid.", "Save a valid HTTP or HTTPS upstream address.")
			continue
		}
		port := u.Port()
		if port == "" {
			port = "80"
			if u.Scheme == "https" {
				port = "443"
			}
		}
		address := net.JoinHostPort(u.Hostname(), port)
		probeCtx, done := context.WithTimeout(ctx, 2*time.Second)
		conn, err := p.dial(probeCtx, "tcp", address)
		if conn != nil {
			conn.Close()
		}
		done()
		if err != nil {
			add("Upstream", site.Domain, "Failed", "Upstream TCP port is unavailable.", "Start the application and verify its host and port.")
		} else if u.Scheme == "https" {
			probeCtx, done := context.WithTimeout(ctx, 2*time.Second)
			err := p.tls(probeCtx, address, u.Hostname())
			done()
			if err != nil {
				add("Upstream TLS", site.Domain, "Failed", "Upstream TLS certificate or handshake could not be verified.", "Correct the upstream certificate and hostname; keep certificate verification enabled.")
			} else {
				add("Upstream TLS", site.Domain, "OK", "Upstream port and TLS certificate verified; HTTP response was not tested.", "")
			}
		} else {
			add("Upstream", site.Domain, "OK", "Upstream TCP port is reachable; HTTP response was not tested.", "")
		}
		probeCtx, done = context.WithTimeout(ctx, 2*time.Second)
		err = p.tls(probeCtx, net.JoinHostPort("127.0.0.1", fmt.Sprint(site.Port)), site.Domain)
		done()
		if err != nil {
			add("Proxy TLS", site.Domain, "Failed", "Local HTTPS listener or certificate could not be verified.", "Apply proxy, verify CA trust and the HTTPS port, then run diagnostics again.")
		} else {
			add("Proxy TLS", site.Domain, "OK", "Local HTTPS listener and domain certificate verified using system trust.", "")
		}
	}
	return r, ctx.Err()
}
