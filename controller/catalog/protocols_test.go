package main

import (
	"context"
	"encoding/json"
	"net/http"
	"reflect"
	"testing"
)

func TestProtocolOverrideAndFallback(t *testing.T) {
	p := []int{8443, 443}
	entries := map[string]ProtocolEntry{"WireGuardTLS": {IPv4: "192.0.2.2", Ports: &p}}
	ip, ports, explicit, ok := resolveProtocol("192.0.2.1", entries, "stealth")
	if !ok || !explicit || ip != "192.0.2.2" || !reflect.DeepEqual(ports, p) {
		t.Fatal(ip, ports, explicit, ok)
	}
	ip, _, explicit, ok = resolveProtocol("192.0.2.1", nil, "stealth")
	if !ok || explicit || ip != "192.0.2.1" {
		t.Fatal("legacy fallback lost")
	}
	entries["WireGuardTLS"] = ProtocolEntry{Ports: &p}
	ip, ports, _, ok = resolveProtocol("192.0.2.1", entries, "stealth")
	if !ok || ip != "192.0.2.1" || !reflect.DeepEqual(ports, p) {
		t.Fatal("port override must survive address fallback")
	}
	bad := []int{0}
	entries["WireGuardTLS"] = ProtocolEntry{IPv4: "192.0.2.2", Ports: &bad}
	if _, _, _, ok := resolveProtocol("192.0.2.1", entries, "stealth"); ok {
		t.Fatal("invalid explicit ports accepted")
	}
}
func TestMetadataEnrichmentPreservesRankingAndCredentials(t *testing.T) {
	c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/vpn/v2/logicals" || r.URL.Query().Get("WithEntriesForProtocols") == "" {
			t.Fatal(r.URL)
		}
		json.NewEncoder(w).Encode(map[string]any{"Code": 1000, "LogicalServers": []Logical{{Servers: []Physical{{ID: "physical", EntryPerProtocol: map[string]ProtocolEntry{"WireGuardTLS": {IPv4: "192.0.2.2"}}}}}}})
	})
	in := []Logical{{Name: "server", Score: 4, Load: 77, Servers: []Physical{{ID: "physical", EntryIP: "192.0.2.1", X25519PublicKey: "unchanged"}}}}
	out := c.withProtocolEntries(context.Background(), in)
	if out[0].Score != 4 || out[0].Load != 77 || out[0].Servers[0].EntryIP != "192.0.2.1" || out[0].Servers[0].X25519PublicKey != "unchanged" || out[0].Servers[0].EntryPerProtocol["WireGuardTLS"].IPv4 != "192.0.2.2" {
		t.Fatal(out)
	}
}
