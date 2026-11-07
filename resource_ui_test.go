package main

import (
	"context"
	"errors"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
)

func TestUIResourceOnlyNavigation(t *testing.T) {
	a := designFixture(t)
	root := t.TempDir()
	a.dev.managerFactory = func() *devplatform.Manager {
		m := devplatform.New(root)
		m.Run = func(context.Context, string, ...string) (string, error) {
			return "", errors.New("mocked external command")
		}
		return m
	}
	if a.dev.tab != devTabSoftware {
		t.Fatal("default tab is not Software")
	}
	tt := ui.NewTester(a.view, 900, 520)
	tt.Frame()
	all := strings.Join(tt.Texts(), "\n")
	for _, label := range []string{"Software", "Databases", "HTTPS"} {
		if _, ok := tt.Find(label); !ok {
			t.Fatal("missing entry", label)
		}
	}
	for _, removed := range []string{"Projects", "Import a project", "PHP", "Site name", "New HTTPS site"} {
		if strings.Contains(all, removed) {
			t.Fatal("obsolete control", removed)
		}
	}
	if e := tt.Click("Databases"); e != nil {
		t.Fatal(e)
	}
	tt.Frame()
	if e := tt.Click("New database instance"); e != nil {
		t.Fatal(e)
	}
	tt.Frame()
	a.dev.dbID = "draft-db"
	a.dev.dbKind = "redis"
	a.dev.dbPort = "56379"
	if e := tt.Click("HTTPS"); e != nil {
		t.Fatal(e)
	}
	tt.Frame()
	a.dev.proxyDraft = []proxyRuleDraft{{Domain: "draft.test", LocalPort: "3000"}}
	a.dev.proxyDraftLoaded = true
	tt.Frame()
	if strings.Count(strings.Join(tt.Texts(), "\n"), "HTTPS proxy rules") != 1 {
		t.Fatal("HTTPS has multiple editors")
	}
	if e := tt.Click("Databases"); e != nil {
		t.Fatal(e)
	}
	tt.Frame()
	if a.dev.dbID != "draft-db" || a.dev.dbPort != "56379" {
		t.Fatal("database draft lost on tab change")
	}
	if e := tt.Click("HTTPS"); e != nil {
		t.Fatal(e)
	}
	tt.Frame()
	if len(a.dev.proxyDraft) != 1 || a.dev.proxyDraft[0].Domain != "draft.test" {
		t.Fatal("proxy draft lost on tab change")
	}
}

func TestUIHTTPSRulesCanSaveSuccessiveRevisions(t *testing.T) {
	h := newDevUIHarness(t)
	h.click("HTTPS")
	h.fill("HTTPS listener port", "8443")
	h.click("Add rule")
	h.fill("Domain", "first.test")
	h.fill("Local port", "3000")
	h.click("Save HTTPS rules")
	if h.a.dev.proxyDraftRevision != 1 {
		t.Fatal("first save did not refresh revision", h.a.dev.proxyDraftRevision, h.a.dev.output)
	}
	h.fill("Domain", "second.test")
	h.fill("Local port", "4000")
	h.click("Save HTTPS rules")
	state, err := devplatform.New(h.root).Read()
	if err != nil || state.ProxyRulesRevision != 2 || len(state.ProxyRules) != 1 || state.ProxyRules[0].Domain != "second.test" || state.ProxyRules[0].LocalPort != 4000 {
		t.Fatal(state, err, h.a.dev.output)
	}
}

func TestUIHTTPSRemoveFirstRuleKeepsFollowingRule(t *testing.T) {
	a := designFixture(t)
	a.dev.tab = devTabHTTPS
	a.dev.proxyDraft = []proxyRuleDraft{{Domain: "first.test", LocalPort: "3000"}, {Domain: "second.test", LocalPort: "4000"}}
	a.dev.proxyDraftLoaded = true
	tt := ui.NewTester(a.view, 900, 520)
	tt.Frame()
	if e := tt.Click("Remove rule"); e != nil {
		t.Fatal(e)
	}
	tt.Frame()
	if len(a.dev.proxyDraft) != 1 || a.dev.proxyDraft[0].Domain != "second.test" {
		t.Fatal(a.dev.proxyDraft)
	}
}
