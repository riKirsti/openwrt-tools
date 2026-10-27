package main

import (
	"encoding/base64"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestRecommendation(t *testing.T) {
	key := base64.StdEncoding.EncodeToString(make([]byte, 32))
	ep := Endpoint{EntryIP: "1.2.3.4", PublicKey: key, Online: true}
	nodes := []Node{{Name: "slow", Tier: "Free", Score: 9, Features: []string{"IPv6"}, Endpoints: []Endpoint{ep, ep}}, {Name: "best", Tier: "Free", Score: 1, Features: []string{"IPv6"}, Endpoints: []Endpoint{ep}}, {Name: "paid", Tier: "Plus", Score: 0, Features: []string{"IPv6"}, Endpoints: []Endpoint{ep}}, {Name: "v4", Tier: "Free", Score: 0, Endpoints: []Endpoint{ep}}}
	r := candidates(nodes)
	if len(r) != 2 || r[0].Name != "best" || len(r[1].Endpoints) != 1 {
		t.Fatal("incorrect account/IPv6 recommendation")
	}
}
func TestNetworkAndModeIndependence(t *testing.T) {
	a := networkID(Network{Kind: "wifi", SSID: "A", GatewayMAC: "00:11:22:33:44:55"})
	b := networkID(Network{Kind: "wifi", SSID: "B", GatewayMAC: "00:11:22:33:44:55"})
	s := Store{}
	nodes := []Node{{Name: "choice"}}
	if !preference(s, a, "local").WireGuard || preference(s, a, "home").Server != "auto" {
		t.Fatal("bad defaults")
	}
	if e := set(s, a, "local", "wireguard", "off", nodes); e != nil {
		t.Fatal(e)
	}
	if e := set(s, a, "local", "server", "choice", nodes); e != nil {
		t.Fatal(e)
	}
	if preference(s, a, "local").WireGuard || preference(s, a, "local").Server != "choice" || !preference(s, a, "home").WireGuard || preference(s, b, "local").Server != "auto" {
		t.Fatal("preferences crossed mode/network boundary")
	}
	if e := set(s, a, "home", "server", "invalid", nodes); e == nil {
		t.Fatal("unknown node accepted")
	}
	if e := set(s, a, "direct", "wireguard", "off", nodes); e == nil {
		t.Fatal("Direct Internet Node accepted a toggle")
	}
	path := filepath.Join(t.TempDir(), "state.json")
	if e := save(path, s); e != nil {
		t.Fatal(e)
	}
	info, _ := os.Stat(path)
	if runtime.GOOS != "windows" && info.Mode().Perm() != 0600 {
		t.Fatal("permissions")
	}
	s2, e := load(path)
	if e != nil || preference(s2, a, "local").Server != "choice" {
		t.Fatal("persistence")
	}
	raw, _ := os.ReadFile(path)
	if strings.Contains(string(raw), "SSID") {
		t.Fatal("raw network identity persisted")
	}
}
func TestDHCPAddressDoesNotResetMemory(t *testing.T) {
	a := Network{Kind: "wired", GatewayMAC: "aa:bb:cc:dd:ee:ff", GatewayIP: "192.168.1.1"}
	b := a
	b.GatewayIP = "10.0.0.1"
	if networkID(a) != networkID(b) {
		t.Fatal("address change reset stable network")
	}
}

func TestWireGuardSwitchPersistsPerModeAcrossNetworks(t *testing.T) {
	s := Store{}
	if e := set(s, "first", "local", "wireguard", "off", nil); e != nil {
		t.Fatal(e)
	}
	if preference(s, "second", "local").WireGuard || !preference(s, "second", "home").WireGuard {
		t.Fatal("WireGuard switches must be independent per mode")
	}
}
