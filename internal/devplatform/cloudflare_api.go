package devplatform

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"net/textproto"
	"net/url"
	"sort"
	"strings"
	"time"
)

const (
	defaultCloudflareAPIBase   = "https://api.cloudflare.com/client/v4"
	cloudflareScriptDateHeader = "2025-08-01"
	cloudflareWorkerCompatDate = "2026-04-07"
)

func currentTunnelWorkerDigest() string {
	sum := sha256.Sum256([]byte(cloudflareWorkerCompatDate + "\n" + cloudflareWorkerIndexMJS))
	return hex.EncodeToString(sum[:8])
}

type cfAPIError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

type cloudflareAPIError struct {
	StatusCode int
	Errors     []cfAPIError
	Body       string
}

func (e *cloudflareAPIError) Error() string {
	if len(e.Errors) > 0 {
		parts := make([]string, 0, len(e.Errors))
		for _, item := range e.Errors {
			if item.Code != 0 {
				parts = append(parts, fmt.Sprintf("%s (code %d)", item.Message, item.Code))
			} else if item.Message != "" {
				parts = append(parts, item.Message)
			}
		}
		if len(parts) > 0 {
			return fmt.Sprintf("Cloudflare API error (%d): %s", e.StatusCode, strings.Join(parts, "; "))
		}
	}
	if strings.TrimSpace(e.Body) != "" {
		return fmt.Sprintf("Cloudflare API error (%d): %s", e.StatusCode, strings.TrimSpace(e.Body))
	}
	return fmt.Sprintf("Cloudflare API error (%d)", e.StatusCode)
}

func hasCFErrorCode(err error, statusCode int, codes ...int) bool {
	var apiErr *cloudflareAPIError
	if !errors.As(err, &apiErr) {
		return false
	}
	if statusCode > 0 && apiErr.StatusCode == statusCode {
		return true
	}
	for _, want := range codes {
		for _, item := range apiErr.Errors {
			if item.Code == want {
				return true
			}
		}
	}
	return false
}

type cfEnvelope[T any] struct {
	Success bool         `json:"success"`
	Errors  []cfAPIError `json:"errors"`
	Result  T            `json:"result"`
}

type cloudflareClient struct {
	baseURL string
	token   string
	http    *http.Client
}

func newCloudflareClient(baseURL, token string) *cloudflareClient {
	base := strings.TrimRight(strings.TrimSpace(baseURL), "/")
	if base == "" {
		base = defaultCloudflareAPIBase
	}
	return &cloudflareClient{
		baseURL: base,
		token:   token,
		http:    &http.Client{Timeout: 30 * time.Second},
	}
}

func cfDoJSON[T any](ctx context.Context, c *cloudflareClient, method, path string, body any, headers map[string]string) (T, error) {
	var zero T
	var payload io.Reader
	if body != nil {
		b, err := json.Marshal(body)
		if err != nil {
			return zero, err
		}
		payload = bytes.NewReader(b)
	}
	req, err := http.NewRequestWithContext(ctx, method, c.baseURL+path, payload)
	if err != nil {
		return zero, err
	}
	req.Header.Set("Authorization", "Bearer "+c.token)
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	return executeCFRequest[T](c.http, req)
}

func executeCFRequest[T any](client *http.Client, req *http.Request) (T, error) {
	var zero T
	resp, err := client.Do(req)
	if err != nil {
		return zero, err
	}
	defer resp.Body.Close()

	raw, err := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
	if err != nil {
		return zero, err
	}
	var env cfEnvelope[T]
	if len(raw) > 0 {
		if err := json.Unmarshal(raw, &env); err != nil {
			if resp.StatusCode >= 400 {
				return zero, &cloudflareAPIError{StatusCode: resp.StatusCode, Body: string(raw)}
			}
			return zero, fmt.Errorf("decode Cloudflare response: %w", err)
		}
	}
	if resp.StatusCode >= 400 || !env.Success {
		return zero, &cloudflareAPIError{
			StatusCode: resp.StatusCode,
			Errors:     env.Errors,
			Body:       string(raw),
		}
	}
	return env.Result, nil
}

func (c *cloudflareClient) discoverAccountID(ctx context.Context) (string, error) {
	accounts, err := cfDoJSON[[]struct {
		ID   string `json:"id"`
		Name string `json:"name"`
	}](ctx, c, http.MethodGet, "/accounts?per_page=20", nil, nil)
	if err == nil && len(accounts) > 0 && accounts[0].ID != "" {
		return accounts[0].ID, nil
	}
	zones, zErr := cfDoJSON[[]struct {
		ID      string `json:"id"`
		Name    string `json:"name"`
		Account struct {
			ID string `json:"id"`
		} `json:"account"`
	}](ctx, c, http.MethodGet, "/zones?status=active&per_page=50", nil, nil)
	if zErr == nil {
		for _, z := range zones {
			if z.Account.ID != "" {
				return z.Account.ID, nil
			}
		}
	}
	if err != nil {
		return "", err
	}
	return "", errors.New("no Cloudflare account found for API token; enter Account ID manually")
}

func (c *cloudflareClient) getWorkersSubdomain(ctx context.Context, accountID string) (string, error) {
	res, err := cfDoJSON[struct {
		Subdomain string `json:"subdomain"`
	}](ctx, c, http.MethodGet, fmt.Sprintf("/accounts/%s/workers/subdomain", url.PathEscape(accountID)), nil, nil)
	if err != nil {
		if hasCFErrorCode(err, http.StatusNotFound, 10007) {
			return "", nil
		}
		return "", err
	}
	return strings.ToLower(strings.TrimSpace(res.Subdomain)), nil
}

func (c *cloudflareClient) listZones(ctx context.Context) ([]CloudflareZone, error) {
	rawZones, err := cfDoJSON[[]struct {
		ID   string `json:"id"`
		Name string `json:"name"`
	}](ctx, c, http.MethodGet, "/zones?status=active&per_page=50", nil, nil)
	if err != nil {
		return nil, err
	}
	zones := make([]CloudflareZone, 0, len(rawZones))
	for _, z := range rawZones {
		name := strings.ToLower(strings.TrimSpace(z.Name))
		if z.ID == "" || name == "" {
			continue
		}
		zones = append(zones, CloudflareZone{ID: z.ID, Name: name})
	}
	sort.Slice(zones, func(i, j int) bool { return zones[i].Name < zones[j].Name })
	return zones, nil
}

func (c *cloudflareClient) listDNSRecords(ctx context.Context, zoneID string) ([]CloudflareDNSRecord, error) {
	rawRecords, err := cfDoJSON[[]struct {
		ID   string `json:"id"`
		Name string `json:"name"`
		Type string `json:"type"`
	}](ctx, c, http.MethodGet, fmt.Sprintf("/zones/%s/dns_records?per_page=100", url.PathEscape(zoneID)), nil, nil)
	if err != nil {
		return nil, err
	}
	seen := make(map[string]bool, len(rawRecords))
	records := make([]CloudflareDNSRecord, 0, len(rawRecords))
	for _, r := range rawRecords {
		typ := strings.ToUpper(strings.TrimSpace(r.Type))
		switch typ {
		case "A", "AAAA", "CNAME":
		default:
			continue
		}
		name := strings.ToLower(strings.TrimSpace(r.Name))
		if name == "" || seen[name+":"+typ] {
			continue
		}
		seen[name+":"+typ] = true
		records = append(records, CloudflareDNSRecord{
			ID:     r.ID,
			ZoneID: zoneID,
			Name:   name,
			Type:   typ,
		})
	}
	sort.Slice(records, func(i, j int) bool {
		if records[i].Name != records[j].Name {
			return records[i].Name < records[j].Name
		}
		return records[i].Type < records[j].Type
	})
	return records, nil
}

func (c *cloudflareClient) getScriptMigrationTag(ctx context.Context, accountID, scriptName string) (string, error) {
	res, err := cfDoJSON[struct {
		DefaultEnvironment struct {
			Script struct {
				MigrationTag string `json:"migration_tag"`
			} `json:"script"`
		} `json:"default_environment"`
	}](ctx, c, http.MethodGet, fmt.Sprintf("/accounts/%s/workers/services/%s", url.PathEscape(accountID), url.PathEscape(scriptName)), nil, nil)
	if err != nil {
		if hasCFErrorCode(err, http.StatusNotFound, 10007, 10092) {
			return "", nil
		}
		return "", err
	}
	return strings.TrimSpace(res.DefaultEnvironment.Script.MigrationTag), nil
}

func (c *cloudflareClient) putWorkerScript(ctx context.Context, accountID, scriptName, tunnelSecret, accessToken, existingMigrationTag string) error {
	if !strings.HasPrefix(scriptName, "devbase-") {
		return fmt.Errorf("refusing to deploy non-devbase worker %q", scriptName)
	}
	bindings := []map[string]any{
		{
			"name":       "TUNNEL_GATEWAY",
			"type":       "durable_object_namespace",
			"class_name": "TunnelGateway",
		},
		{
			"name": "TUNNEL_SECRET",
			"type": "secret_text",
			"text": tunnelSecret,
		},
	}
	if accessToken != "" {
		bindings = append(bindings, map[string]any{
			"name": "ACCESS_TOKEN",
			"type": "secret_text",
			"text": accessToken,
		})
	}
	meta := map[string]any{
		"main_module":        "index.mjs",
		"compatibility_date": cloudflareWorkerCompatDate,
		"bindings":           bindings,
	}
	if existingMigrationTag != "v1" {
		mig := map[string]any{
			"new_tag": "v1",
			"steps": []map[string]any{
				{"new_sqlite_classes": []string{"TunnelGateway"}},
			},
		}
		if existingMigrationTag != "" {
			mig["old_tag"] = existingMigrationTag
		}
		meta["migrations"] = mig
	}
	metaJSON, err := json.Marshal(meta)
	if err != nil {
		return err
	}

	var body bytes.Buffer
	mw := multipart.NewWriter(&body)

	metaHeader := make(textproto.MIMEHeader)
	metaHeader.Set("Content-Disposition", `form-data; name="metadata"`)
	metaHeader.Set("Content-Type", "application/json")
	metaPart, err := mw.CreatePart(metaHeader)
	if err != nil {
		return err
	}
	if _, err := metaPart.Write(metaJSON); err != nil {
		return err
	}

	modHeader := make(textproto.MIMEHeader)
	modHeader.Set("Content-Disposition", `form-data; name="index.mjs"; filename="index.mjs"`)
	modHeader.Set("Content-Type", "application/javascript+module")
	modPart, err := mw.CreatePart(modHeader)
	if err != nil {
		return err
	}
	if _, err := modPart.Write([]byte(cloudflareWorkerIndexMJS)); err != nil {
		return err
	}
	if err := mw.Close(); err != nil {
		return err
	}

	endpoint := fmt.Sprintf("%s/accounts/%s/workers/scripts/%s", c.baseURL, url.PathEscape(accountID), url.PathEscape(scriptName))
	req, err := http.NewRequestWithContext(ctx, http.MethodPut, endpoint, &body)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+c.token)
	req.Header.Set("Content-Type", mw.FormDataContentType())

	_, err = executeCFRequest[any](c.http, req)
	return err
}

func (c *cloudflareClient) setScriptSubdomainEnabled(ctx context.Context, accountID, scriptName string, enabled bool) error {
	path := fmt.Sprintf("/accounts/%s/workers/scripts/%s/subdomain", url.PathEscape(accountID), url.PathEscape(scriptName))
	headers := map[string]string{
		"Cloudflare-Workers-Script-Api-Date": cloudflareScriptDateHeader,
	}
	_, err := cfDoJSON[any](ctx, c, http.MethodPost, path, map[string]bool{"enabled": enabled}, headers)
	return err
}

func (c *cloudflareClient) setScriptCustomDomains(ctx context.Context, accountID, scriptName, zoneID, hostname string) error {
	origins := make([]map[string]string, 0, 1)
	if hostname != "" {
		origins = append(origins, map[string]string{
			"hostname": hostname,
			"zone_id":  zoneID,
		})
	}
	payload := map[string]any{
		"override_scope":               true,
		"override_existing_origin":     true,
		"override_existing_dns_record": true,
		"origins":                      origins,
	}
	path := fmt.Sprintf("/accounts/%s/workers/scripts/%s/domains/records", url.PathEscape(accountID), url.PathEscape(scriptName))
	_, err := cfDoJSON[any](ctx, c, http.MethodPut, path, payload, nil)
	return err
}

func (c *cloudflareClient) deleteWorkerScript(ctx context.Context, accountID, scriptName string) error {
	if !strings.HasPrefix(scriptName, "devbase-") {
		return fmt.Errorf("refusing to delete non-devbase worker %q", scriptName)
	}
	if err := c.setScriptCustomDomains(ctx, accountID, scriptName, "", ""); err != nil {
		if !hasCFErrorCode(err, http.StatusNotFound, 10007) {
			return err
		}
	}
	path := fmt.Sprintf("/accounts/%s/workers/scripts/%s?force=true", url.PathEscape(accountID), url.PathEscape(scriptName))
	if _, err := cfDoJSON[any](ctx, c, http.MethodDelete, path, nil, nil); err != nil {
		if hasCFErrorCode(err, http.StatusNotFound, 10007) {
			return nil
		}
		return err
	}
	return nil
}

func matchZoneForHostname(hostname string, zones []CloudflareZone) (CloudflareZone, bool) {
	host := strings.ToLower(strings.TrimSpace(hostname))
	var best CloudflareZone
	found := false
	for _, z := range zones {
		name := strings.ToLower(strings.TrimSpace(z.Name))
		if name == "" {
			continue
		}
		if host == name || strings.HasSuffix(host, "."+name) {
			if !found || len(name) > len(best.Name) {
				best = z
				found = true
			}
		}
	}
	return best, found
}

// SaveCloudflareSettings saves the Cloudflare API token (0600 in proxy/cloudflare.key)
// along with optional account and client identifiers without transmitting any secret into state.json.
func (m *Manager) SaveCloudflareSettings(ctx context.Context, apiToken, accountID, clientID string) error {
	if err := m.init(); err != nil {
		return err
	}
	na, err := normalizeCloudflareAccountID(accountID)
	if err != nil {
		return err
	}
	nc, err := normalizeTunnelClientID(clientID)
	if err != nil {
		return err
	}
	var cleanedToken string
	if strings.TrimSpace(apiToken) != "" {
		cleanedToken, err = validateCloudflareToken(apiToken)
		if err != nil {
			return err
		}
	}
	return m.withState(ctx, func(s *State) error {
		if nc == "" && s.TunnelClientID != "" {
			nc = s.TunnelClientID
		}
		if nc == "" && (cleanedToken != "" || na != "" || len(s.TunnelWorkers) > 0) {
			nc = "mac-" + randomID()[:8]
		}
		s.CloudflareAccountID = na
		s.TunnelClientID = nc
		if cleanedToken != "" {
			if err := m.writeCloudflareToken(cleanedToken); err != nil {
				return err
			}
		}
		return nil
	})
}

// RefreshCloudflareMetadata discovers the Cloudflare account ID, workers.dev
// subdomain, active zones, and A/AAAA/CNAME records for custom domain suggestions.
func (m *Manager) RefreshCloudflareMetadata(ctx context.Context) error {
	if err := m.init(); err != nil {
		return err
	}
	token, err := m.ReadCloudflareToken()
	if err != nil {
		return errors.New("configure and save a Cloudflare API token first")
	}
	s, err := m.Read()
	if err != nil {
		return err
	}
	cf := newCloudflareClient(m.CloudflareAPIBase, token)
	accountID, subdomain, zones, dnsRecords, err := fetchCloudflareMetadata(ctx, cf, s.CloudflareAccountID)
	if err != nil {
		return err
	}
	return m.withState(ctx, func(cur *State) error {
		if cur.CloudflareAccountID == "" {
			cur.CloudflareAccountID = accountID
		}
		cur.CloudflareSubdomain = subdomain
		cur.CloudflareZones = zones
		cur.CloudflareDNS = dnsRecords
		if cur.TunnelClientID == "" {
			cur.TunnelClientID = "mac-" + randomID()[:8]
		}
		return nil
	})
}

func fetchCloudflareMetadata(ctx context.Context, cf *cloudflareClient, accountID string) (string, string, []CloudflareZone, []CloudflareDNSRecord, error) {
	resolvedAccountID := strings.TrimSpace(accountID)
	if resolvedAccountID == "" {
		var err error
		resolvedAccountID, err = cf.discoverAccountID(ctx)
		if err != nil {
			return "", "", nil, nil, err
		}
	}
	subdomain, err := cf.getWorkersSubdomain(ctx, resolvedAccountID)
	if err != nil {
		return "", "", nil, nil, err
	}
	zones, err := cf.listZones(ctx)
	if err != nil {
		return "", "", nil, nil, err
	}
	var dnsRecords []CloudflareDNSRecord
	for _, z := range zones {
		recs, err := cf.listDNSRecords(ctx, z.ID)
		if err != nil {
			continue
		}
		dnsRecords = append(dnsRecords, recs...)
	}
	return resolvedAccountID, subdomain, zones, dnsRecords, nil
}

// SyncTunnels reconciles desired per-rule Cloudflare Workers with deployed cloud
// state: deploying or updating enabled rules, attaching Custom Domains or
// workers.dev URLs, deleting orphaned devbase-* Workers, and refreshing tunnel.json.
func (m *Manager) SyncTunnels(ctx context.Context) error {
	if err := m.init(); err != nil {
		return err
	}
	s, err := m.Read()
	if err != nil {
		return err
	}

	desired := make([]ProxyRule, 0, len(s.ProxyRules))
	desiredWorkers := make(map[string]ProxyRule)
	for _, rule := range s.ProxyRules {
		if !rule.Tunnel {
			continue
		}
		workerName := rule.ResolvedTunnelWorker()
		desired = append(desired, rule)
		desiredWorkers[workerName] = rule
	}

	if len(desired) == 0 && len(s.TunnelWorkers) == 0 {
		return m.withState(ctx, func(cur *State) error {
			return m.writePublishedTunnelConfig(cur)
		})
	}

	token, err := m.ReadCloudflareToken()
	if err != nil {
		return errors.New("configure and save a Cloudflare API token first")
	}
	tunnelSecret, err := m.ensureTunnelSecret()
	if err != nil {
		return err
	}

	cf := newCloudflareClient(m.CloudflareAPIBase, token)
	accountID, subdomain, zones, dnsRecords, err := fetchCloudflareMetadata(ctx, cf, s.CloudflareAccountID)
	if err != nil {
		return err
	}

	// Validate custom domain zone ownership before mutating Cloudflare resources.
	zoneMatches := make(map[string]CloudflareZone, len(desired))
	for _, rule := range desired {
		customHost := rule.EffectiveTunnelDomain()
		if customHost == "" {
			if subdomain == "" {
				return errors.New("register a workers.dev subdomain in Cloudflare Dashboard, or set a Custom domain")
			}
			continue
		}
		zone, ok := matchZoneForHostname(customHost, zones)
		if !ok {
			return fmt.Errorf("custom domain %s does not belong to any active Cloudflare zone on this account", customHost)
		}
		zoneMatches[rule.ResolvedTunnelWorker()] = zone
	}

	// Remove orphaned Workers first so detached Custom Domains are freed before rebinding.
	orphanNames := make([]string, 0)
	for name := range s.TunnelWorkers {
		if _, keep := desiredWorkers[name]; !keep {
			orphanNames = append(orphanNames, name)
		}
	}
	sort.Strings(orphanNames)
	for _, name := range orphanNames {
		if err := cf.deleteWorkerScript(ctx, accountID, name); err != nil {
			return err
		}
	}

	deployed := make(map[string]TunnelWorkerState, len(desired))
	urlsByLocalDomain := make(map[string]string, len(desired))
	sort.Slice(desired, func(i, j int) bool {
		return desired[i].ResolvedTunnelWorker() < desired[j].ResolvedTunnelWorker()
	})

	for _, rule := range desired {
		workerName := rule.ResolvedTunnelWorker()
		accessToken := ""
		if rule.TunnelAuth {
			accessToken, err = m.EnsureTunnelAccessToken(rule.Domain)
			if err != nil {
				return err
			}
		}
		existingTag, err := cf.getScriptMigrationTag(ctx, accountID, workerName)
		if err != nil {
			return err
		}
		if err := cf.putWorkerScript(ctx, accountID, workerName, tunnelSecret, accessToken, existingTag); err != nil {
			return err
		}

		customHost := rule.EffectiveTunnelDomain()
		var zoneID, publicURL string
		if customHost != "" {
			zone := zoneMatches[workerName]
			zoneID = zone.ID
			if subdomain != "" {
				if err := cf.setScriptSubdomainEnabled(ctx, accountID, workerName, false); err != nil && !hasCFErrorCode(err, http.StatusNotFound, 10007) {
					return err
				}
			}
			if err := cf.setScriptCustomDomains(ctx, accountID, workerName, zoneID, customHost); err != nil {
				return err
			}
			publicURL = "https://" + customHost
		} else {
			if prev, had := s.TunnelWorkers[workerName]; had && prev.CustomDomain != "" {
				if err := cf.setScriptCustomDomains(ctx, accountID, workerName, "", ""); err != nil && !hasCFErrorCode(err, http.StatusNotFound, 10007) {
					return err
				}
			}
			if err := cf.setScriptSubdomainEnabled(ctx, accountID, workerName, true); err != nil {
				return err
			}
			publicURL = fmt.Sprintf("https://%s.%s.workers.dev", workerName, subdomain)
		}

		deployed[workerName] = TunnelWorkerState{
			ScriptName:   workerName,
			PublicURL:    publicURL,
			CustomDomain: customHost,
			ZoneID:       zoneID,
			LocalDomain:  rule.Domain,
			LocalPort:    rule.LocalPort,
			ScriptDigest: currentTunnelWorkerDigest(),
			AuthDigest:   tunnelAuthDigest(accessToken),
		}
		urlsByLocalDomain[rule.Domain] = publicURL
	}

	return m.withState(ctx, func(cur *State) error {
		if cur.CloudflareAccountID == "" {
			cur.CloudflareAccountID = accountID
		}
		cur.CloudflareSubdomain = subdomain
		cur.CloudflareZones = zones
		cur.CloudflareDNS = dnsRecords
		if cur.TunnelClientID == "" {
			cur.TunnelClientID = "mac-" + randomID()[:8]
		}
		cur.TunnelWorkers = deployed
		for i := range cur.ProxyRules {
			if u, ok := urlsByLocalDomain[cur.ProxyRules[i].Domain]; ok {
				cur.ProxyRules[i].TunnelURL = u
			} else if !cur.ProxyRules[i].Tunnel {
				cur.ProxyRules[i].TunnelURL = ""
			}
		}
		return m.writePublishedTunnelConfig(cur)
	})
}
