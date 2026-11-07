package devplatform

import (
	"crypto/sha256"
	"crypto/x509"
	"encoding/json"
	"errors"
	"html/template"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"sync"
	"time"
)

type reverseRoute struct {
	site      Site
	proxy     *httputil.ReverseProxy
	transport *http.Transport
}
type proxySnapshot struct {
	sites  []Site
	routes map[string]*reverseRoute
}
type liveProxyRoutes struct {
	mu      sync.Mutex
	manager *Manager
	digest  [32]byte
	current *proxySnapshot
}

func routeID(site Site) string { return net.JoinHostPort(site.Domain, strconv.Itoa(site.Port)) }

func newLiveProxyRoutes(m *Manager, sites []Site) *liveProxyRoutes {
	routes := &liveProxyRoutes{manager: m}
	routes.current = routes.build(sites, nil)
	return routes
}
func (routes *liveProxyRoutes) build(sites []Site, old *proxySnapshot) *proxySnapshot {
	next := &proxySnapshot{sites: sites, routes: map[string]*reverseRoute{}}
	for _, site := range sites {
		if site.Disabled {
			continue
		}
		id := routeID(site)
		if old != nil {
			if route := old.routes[id]; route != nil && route.site.Upstream == site.Upstream && route.site.ProxyOptions() == site.ProxyOptions() {
				next.routes[id] = route
				continue
			}
		}
		next.routes[id] = newReverseRoute(site)
	}
	if old != nil {
		for id, route := range old.routes {
			if next.routes[id] != route {
				route.transport.CloseIdleConnections()
			}
		}
	}
	return next
}

// Every published snapshot has passed full validation. A corrupt or missing
// file leaves the last good routing table and transports available to requests.
func (routes *liveProxyRoutes) snapshot() *proxySnapshot {
	routes.mu.Lock()
	defer routes.mu.Unlock()
	b, e := os.ReadFile(filepath.Join(routes.manager.Root, "proxy", "sites.json"))
	if e != nil {
		return routes.current
	}
	digest := sha256.Sum256(b)
	if digest == routes.digest {
		return routes.current
	}
	var sites []Site
	if json.Unmarshal(b, &sites) != nil || validateSites(sites) != nil {
		return routes.current
	}
	routes.current = routes.build(sites, routes.current)
	routes.digest = digest
	return routes.current
}
func (routes *liveProxyRoutes) close() {
	routes.mu.Lock()
	defer routes.mu.Unlock()
	for _, route := range routes.current.routes {
		route.transport.CloseIdleConnections()
	}
}

func newReverseRoute(site Site) *reverseRoute {
	target, _ := url.Parse(site.Upstream)
	options := site.ProxyOptions()
	transport := &http.Transport{ForceAttemptHTTP2: true, IdleConnTimeout: 90 * time.Second, MaxIdleConns: 100}
	if defaults, ok := http.DefaultTransport.(*http.Transport); ok {
		transport = defaults.Clone()
	}
	transport.Proxy = nil
	transport.DialContext = (&net.Dialer{Timeout: time.Duration(options.ConnectTimeoutSeconds) * time.Second, KeepAlive: 30 * time.Second}).DialContext
	transport.TLSHandshakeTimeout = time.Duration(options.ConnectTimeoutSeconds) * time.Second
	transport.ResponseHeaderTimeout = time.Duration(options.ResponseHeaderTimeoutSeconds) * time.Second
	proxy := &httputil.ReverseProxy{Transport: transport, FlushInterval: -1}
	proxy.Rewrite = func(request *httputil.ProxyRequest) {
		request.SetURL(target)
		if options.HostPolicy == "local" {
			request.Out.Host = request.In.Host
		}
		request.SetXForwarded()
	}
	proxy.ErrorHandler = func(w http.ResponseWriter, r *http.Request, e error) { writeProxyFailure(w, site, e) }
	return &reverseRoute{site: site, proxy: proxy, transport: transport}
}

var proxyFailurePage = template.Must(template.New("proxy-failure").Parse(`<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>{{.Title}}</title><style>body{font:16px/1.6 system-ui;margin:12vh auto;padding:24px;max-width:640px;color:#202124;background:#f5f6f8}main{background:white;padding:32px;border-radius:16px}h1{line-height:1.2;font-size:28px}code{word-break:break-all}p{color:#555}</style></head><body><main><h1>{{.Title}}</h1><p>{{.Message}}</p><p><strong>{{.Domain}}</strong> → <code>{{.Upstream}}</code></p><p>Open Service Manager to check the project status, upstream address, and service logs.</p></main></body></html>`))

func writeProxyFailure(w http.ResponseWriter, site Site, e error) {
	status, title, message := http.StatusBadGateway, "Local service is unavailable", "The local server did not accept this request. Start the project or check its port."
	var network net.Error
	var authority x509.UnknownAuthorityError
	var hostname x509.HostnameError
	var invalid x509.CertificateInvalidError
	if errors.As(e, &network) && network.Timeout() {
		status, title, message = http.StatusGatewayTimeout, "Local service timed out", "The local server did not respond within this site's timeout. Check its logs or adjust the proxy timeout."
	} else if errors.As(e, &authority) || errors.As(e, &hostname) || errors.As(e, &invalid) {
		title, message = "Upstream certificate is not trusted", "The HTTPS upstream certificate could not be verified. Use a trusted certificate with the correct hostname."
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_ = proxyFailurePage.Execute(w, struct{ Title, Message, Domain, Upstream string }{title, message, site.Domain, site.Upstream})
}
