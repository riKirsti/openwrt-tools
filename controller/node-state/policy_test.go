package main

import (
	"encoding/base64"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

func testKey() string { return base64.StdEncoding.EncodeToString(make([]byte, 32)) }
func TestProtocolEligibility(t *testing.T) {
	t.Setenv("PROTON_PROTOCOL", "stealth")
	ports := []int{8443, 443}
	nodes := []Node{
		{Name: "legacy", Tier: "Free", Features: []string{"IPv6"}, Endpoints: []Endpoint{{EntryIP: "192.0.2.1", PublicKey: testKey(), Online: true}}},
		{Name: "explicit", Tier: "Free", Features: []string{"IPv6"}, Endpoints: []Endpoint{{PublicKey: testKey(), Online: true, EntryPerProtocol: map[string]ProtocolEntry{"WireGuardTLS": {IPv4: "192.0.2.2", Ports: &ports}}}}},
		{Name: "unsupported", Tier: "Free", Features: []string{"IPv6"}, Endpoints: []Endpoint{{PublicKey: testKey(), Online: true, EntryPerProtocol: map[string]ProtocolEntry{"WireGuardUDP": {IPv4: "192.0.2.3"}}}}},
		{Name: "invalid-key", Tier: "Free", Features: []string{"IPv6"}, Endpoints: []Endpoint{{EntryIP: "192.0.2.4", PublicKey: "bad", Online: true}}},
	}
	out := candidates(nodes)
	if len(out) != 2 {
		t.Fatalf("expected legacy fallback and explicit TLS only, got %d", len(out))
	}
	for _, n := range out {
		if n.Protocol != "stealth" || n.ProtocolState != "eligible" {
			t.Fatal(n)
		}
		if n.Name == "explicit" && n.Endpoints[0].EntryIP != "192.0.2.2" {
			t.Fatal("did not resolve TLS address")
		}
	}
}
func TestScopedObservationsAndManualChoice(t *testing.T) {
	t.Setenv("PROTON_PROTOCOL", "stealth")
	p := filepath.Join(t.TempDir(), "history.json")
	key := testKey()
	finger := endpointFingerprint("192.0.2.1", key)
	h := History{1, bootID(), map[string]Observation{historyKey("network-a", "stealth", "A"): {Success: 1000, SuccessEndpoint: finger, Port: 443}, historyKey("network-a", "stealth", "B"): {Failure: 1050, FailureEndpoint: finger}}}
	if e := writeHistory(p, h); e != nil {
		t.Fatal(e)
	}
	nodes := []Node{{Name: "A", Protocol: "stealth", ProtocolState: "eligible", Endpoints: []Endpoint{{EntryIP: "192.0.2.1", PublicKey: key}}}, {Name: "B", Protocol: "stealth", ProtocolState: "eligible", Endpoints: []Endpoint{{EntryIP: "192.0.2.1", PublicKey: key}}}, {Name: "C", Protocol: "stealth", ProtocolState: "eligible"}}
	applyObservations(nodes, "network-a", p, 1100)
	if nodes[0].ProtocolState != "verified" || nodes[1].ProtocolState != "unreachable" {
		t.Fatal(nodes)
	}
	if got := deprioritizeFailures([]string{"B", "A", "C"}, nodes); !reflect.DeepEqual(got, []string{"A", "C", "B"}) {
		t.Fatal(got)
	}
	s := Store{}
	if e := set(s, "network-a", "local", "server", "B", nodes); e != nil || preference(s, "network-a", "local").Server != "B" {
		t.Fatal("cooldown blocked manual selection", e)
	}
	for _, n := range nodes {
		n.ProtocolState = "eligible"
		copy := []Node{n}
		applyObservations(copy, "network-b", p, 1100)
		if copy[0].ProtocolState != "eligible" {
			t.Fatal("observation crossed networks")
		}
	}
	nodes[1].ProtocolState = "eligible"
	applyObservations(nodes, "network-a", p, 1400)
	if nodes[1].ProtocolState == "unreachable" {
		t.Fatal("failure did not expire")
	}
	nodes[0].ProtocolState = "eligible"
	nodes[0].Endpoints[0].EntryIP = "192.0.2.99"
	applyObservations(nodes, "network-a", p, 1100)
	if nodes[0].ProtocolState == "verified" {
		t.Fatal("verification survived endpoint replacement")
	}
}
func TestAllCoolingServersRemainCandidates(t *testing.T) {
	n := []Node{{Name: "A", ProtocolState: "unreachable"}, {Name: "B", ProtocolState: "unreachable"}}
	want := []string{"A", "B"}
	if got := deprioritizeFailures(want, n); !reflect.DeepEqual(got, want) {
		t.Fatal(got)
	}
}
func TestHistoryRejectsOtherBootAndCorruption(t *testing.T) {
	p := filepath.Join(t.TempDir(), "history")
	writeHistory(p, History{1, "different-boot", map[string]Observation{"x": {Success: 1}}})
	if len(readHistory(p, bootID()).Records) != 0 {
		t.Fatal("old boot accepted")
	}
	os.WriteFile(p, []byte("not JSON"), 0600)
	if len(readHistory(p, bootID()).Records) != 0 {
		t.Fatal("corrupt history accepted")
	}
}

func TestSuccessfulRetryClearsFailureAndProtocolIsolation(t *testing.T) {
	p := filepath.Join(t.TempDir(), "history.json")
	t.Setenv("PROTON_HISTORY_PATH", p)
	network := strings.Repeat("a", 64)
	args := []string{"failure", network, "stealth", "A", "192.0.2.1", testKey(), "0"}
	if e := observeCLI(args); e != nil {
		t.Fatal(e)
	}
	args[0], args[6] = "success", "443"
	if e := observeCLI(args); e != nil {
		t.Fatal(e)
	}
	nodes := []Node{{Name: "A", Protocol: "stealth", Endpoints: []Endpoint{{EntryIP: "192.0.2.1", PublicKey: testKey()}}}}
	applyObservations(nodes, network, p, time.Now().Unix())
	if nodes[0].ProtocolState != "verified" {
		t.Fatal("successful retry did not clear cooldown", nodes)
	}
	nodes[0].Protocol = "wireguard-udp"
	applyObservations(nodes, network, p, time.Now().Unix())
	if nodes[0].ProtocolState != "eligible" || nodes[0].VerifiedAt != 0 {
		t.Fatal("verification crossed protocols")
	}
}
