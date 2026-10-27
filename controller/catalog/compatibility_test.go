package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"testing"
	"time"
)

func TestCurrentScriptOptions(t *testing.T) {
	for _, args := range [][]string{
		{"--username", "fixture", "--non-interactive", "--list-servers", "--free-only", "--json"},
		{"--username", "fixture", "--non-interactive", "--server", "US-FREE#1", "--free-only", "--ipv6", "--no-save", "--duration", "7d", "--output", "pending.conf"},
		{"--username", "fixture", "--non-interactive", "--revoke-serial", "123456789"},
		{"--list-servers", "--free-only", "--json"},
	} {
		if _, e := parseOptions(args); e != nil {
			t.Fatal(args, e)
		}
	}
}
func TestInputValidation(t *testing.T) {
	for _, args := range [][]string{{"--non-interactive", "--password", "fixture"}, {"--port-forwarding", "--moderate-nat"}, {"--netshield", "3"}, {"--no-save", "--duration", "8d"}, {"--countries", "U1"}, {"--dns", "bad"}, {"--allowed-ips", "bad"}, {"--session-duration", "-1h"}} {
		if _, e := parseOptions(args); e == nil {
			t.Fatal("invalid options accepted", args)
		}
	}
	for _, s := range []string{"", "12345", "12345a", "1234567"} {
		if validTOTP(s) {
			t.Fatal("bad TOTP accepted")
		}
	}
	for _, s := range []string{"", "one\ntwo", "one\r", "x" + strings.Repeat("x", 4096)} {
		if _, e := readPassword(strings.NewReader(s)); e == nil {
			t.Fatal("bad password input accepted")
		}
	}
	p, e := readPassword(strings.NewReader(" leading and trailing \r\n"))
	if e != nil || string(p) != " leading and trailing " {
		t.Fatal("password whitespace changed", e)
	}
}
func TestVerificationDoesNotDecodeCatalog(t *testing.T) {
	c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprint(w, `NOT JSON; verification only needs status`)
	})
	if e := c.verify(context.Background(), true); e != nil {
		t.Fatal(e)
	}
}
func TestHumanVerificationMetadataAndReplay(t *testing.T) {
	c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("x-pm-human-verification-token") != "synthetic:solved" || r.Header.Get("x-pm-human-verification-token-type") != "captcha" {
			t.Error("missing replay headers")
		}
		w.WriteHeader(422)
		fmt.Fprint(w, `{"Code":9001,"Details":{"HumanVerificationToken":"synthetic-challenge","HumanVerificationMethods":["captcha"]}}`)
	})
	c.hvToken = "synthetic:solved"
	e := c.call(context.Background(), "POST", "/auth", map[string]string{"Username": "fixture"}, nil, false)
	a, ok := e.(*apiError)
	if !ok || a.Challenge != "synthetic-challenge" || len(a.Methods) != 1 {
		t.Fatal("challenge metadata missing")
	}
	if strings.Contains(e.Error(), "synthetic-challenge") || strings.Contains(e.Error(), "synthetic:solved") {
		t.Fatal("verification secrets echoed in generic log")
	}
}
func TestCountryPublicationFromExistingResponse(t *testing.T) {
	published := ""
	n := 0
	c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
		n++
		if r.URL.Path == "/vpn/v1/location" {
			fmt.Fprint(w, `{"Code":1000,"IP":"192.0.2.77","Country":"CA"}`)
		} else {
			if r.Header.Get("X-PM-netzone") != "192.0.2.0" {
				t.Error("wrong netzone")
			}
			fmt.Fprint(w, `{"Code":1000,"LogicalServers":[]}`)
		}
	})
	c.publishCountry = func(s string) { published = s }
	if _, e := c.servers(context.Background()); e != nil {
		t.Fatal(e)
	}
	if published != "CA" || n != 2 {
		t.Fatal("country publication added request or lost country")
	}
}
func TestCertificatePaginationAndFeatures(t *testing.T) {
	requests := 0
	c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
		requests++
		certs := []map[string]any{}
		if r.URL.Query().Get("WithSessions") != "1" {
			t.Error("session certificates excluded")
		}
		if requests == 1 {
			for i := 0; i < 50; i++ {
				certs = append(certs, map[string]any{"SerialNumber": fmt.Sprint(i), "Features": []any{}})
			}
		} else {
			if r.URL.Query().Get("BeginID") != "49" {
				t.Error("wrong cursor")
			}
			certs = append(certs, map[string]any{"SerialNumber": "50", "Features": map[string]any{"RandomNAT": false, "SplitTCP": true}})
		}
		json.NewEncoder(w).Encode(map[string]any{"Code": 1000, "Certificates": certs})
	})
	certs, e := c.certificates(context.Background(), true)
	if e != nil || len(certs) != 51 {
		t.Fatal("pagination failure", e)
	}
	f := featureJSON(certs[50].Features)
	if f["moderate_nat"] != true || f["accelerator"] != true {
		t.Fatal("features lost")
	}
	if featureJSON(certs[0].Features) != nil {
		t.Fatal("empty array misinterpreted")
	}
}
func TestCertificateFeaturesAndPersistentMode(t *testing.T) {
	c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
		var b map[string]any
		json.NewDecoder(r.Body).Decode(&b)
		if b["Mode"] != "persistent" || b["DeviceName"] != "fixture-device" || b["Duration"] != "1440 min" {
			t.Error("wrong persistent request")
		}
		f := b["Features"].(map[string]any)
		if f["RandomNAT"] != false || f["NetShieldLevel"] != float64(2) {
			t.Error("wrong features")
		}
		json.NewEncoder(w).Encode(map[string]any{"Code": 1000, "SerialNumber": "1234", "ExpirationTime": time.Now().Add(time.Hour).Unix()})
	})
	if _, e := c.certificateFor(context.Background(), "synthetic-public-pem", options{duration: "1d", device: "fixture-device", moderate: true, netshield: 2}); e != nil {
		t.Fatal(e)
	}
}
func TestSelectionMatchesCurrentFilters(t *testing.T) {
	all := []Logical{{Name: "A", ExitCountry: "US", Tier: 0, Status: 1, Score: 2, Load: 20, Servers: []Physical{{Status: 1}}}, {Name: "B", ExitCountry: "US", Tier: 0, Status: 1, Score: 2, Load: 10, Servers: []Physical{{Status: 1}}}, {Name: "PLUS", ExitCountry: "CA", Tier: 2, Status: 1, Features: 4, Score: 1, Servers: []Physical{{Status: 1}}}}
	s, e := selectLogical(all, options{free: true, countries: "US"})
	if e != nil || s.Name != "B" {
		t.Fatal("score/load selection failed")
	}
	rows, e := outputServers(all, options{p2p: true})
	if e != nil || len(rows) != 1 || rows[0].Name != "PLUS" {
		t.Fatal("paid P2P filter failed")
	}
	if _, e = selectLogical(all, options{free: true, countries: "NL"}); e == nil {
		t.Fatal("country filter ignored")
	}
}
