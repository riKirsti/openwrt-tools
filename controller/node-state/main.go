package main

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"time"
)

type Endpoint struct {
	EntryIP          string                   `json:"entry_ip"`
	PublicKey        string                   `json:"public_key"`
	Online           bool                     `json:"online"`
	EntryPerProtocol map[string]ProtocolEntry `json:"entry_per_protocol,omitempty"`
}
type Node struct {
	Protocols          []string   `json:"protocols"`
	Protocol           string     `json:"protocol"`
	ProtocolState      string     `json:"protocol_state"`
	VerifiedAt         int64      `json:"verified_at,omitempty"`
	RetryAfter         int64      `json:"retry_after,omitempty"`
	Name               string     `json:"name"`
	Country            string     `json:"country"`
	City               string     `json:"city"`
	Tier               string     `json:"tier"`
	Score              float64    `json:"score"`
	Load               int        `json:"load"`
	Features           []string   `json:"features"`
	Endpoints          []Endpoint `json:"endpoints"`
	LatencyMS          *float64   `json:"latency_ms,omitempty"`
	LatencyMethod      string     `json:"latency_method,omitempty"`
	RecommendationRank *int       `json:"recommendation_rank,omitempty"`
}
type Preference struct {
	WireGuard bool   `json:"wireguard"`
	Server    string `json:"server"`
}
type Network struct {
	Kind       string `json:"kind"`
	SSID       string `json:"ssid,omitempty"`
	GatewayMAC string `json:"gateway_mac,omitempty"`
	GatewayIP  string `json:"gateway_ip,omitempty"`
}
type Store map[string]map[string]Preference

func networkID(n Network) string {
	if n.GatewayMAC != "" {
		n.GatewayIP = ""
	}
	b, _ := json.Marshal(n)
	return fmt.Sprintf("%x", sha256.Sum256(b))
}
func candidates(nodes []Node) []Node {
	out := []Node{}
	for _, n := range nodes {
		if n.Tier != "Free" {
			continue
		}
		v6 := false
		for _, f := range n.Features {
			if f == "IPv6" {
				v6 = true
			}
		}
		if !v6 {
			continue
		}
		eps := []Endpoint{}
		seen := map[string]bool{}
		for _, e := range n.Endpoints {
			key, err := base64.StdEncoding.DecodeString(e.PublicKey)
			resolved, _, _, ok := resolveProtocol(e.EntryIP, e.EntryPerProtocol, os.Getenv("PROTON_PROTOCOL"))
			if e.Online && ok && err == nil && len(key) == 32 && !seen[resolved+e.PublicKey] {
				e.EntryIP = resolved
				eps = append(eps, e)
				seen[e.EntryIP+e.PublicKey] = true
			}
		}
		if len(eps) > 0 {
			n.Protocol = protocolName(os.Getenv("PROTON_PROTOCOL"))
			n.ProtocolState = "eligible"
			n.Protocols = supportedProtocols(n.Endpoints)
			n.Endpoints = eps
			out = append(out, n)
		}
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Score != out[j].Score {
			return out[i].Score < out[j].Score
		}
		if out[i].Load != out[j].Load {
			return out[i].Load < out[j].Load
		}
		return out[i].Name < out[j].Name
	})
	return out
}
func preference(s Store, id, mode string) Preference {
	p := Preference{true, "auto"}
	if m, ok := s[id]; ok {
		if saved, ok := m[mode]; ok {
			p.Server = saved.Server
		}
	}
	if m, ok := s["_modes"]; ok {
		if saved, ok := m[mode]; ok {
			p.WireGuard = saved.WireGuard
		}
	}
	return p
}
func set(s Store, id, mode, field, value string, nodes []Node) error {
	if mode != "local" && mode != "home" {
		return fmt.Errorf("mode must be Local Internet Node or Home Internet Node")
	}
	p := preference(s, id, mode)
	switch field {
	case "wireguard":
		if value != "on" && value != "off" {
			return fmt.Errorf("invalid WireGuard value")
		}
		p.WireGuard = value == "on"
		if s["_modes"] == nil {
			s["_modes"] = map[string]Preference{}
		}
		s["_modes"][mode] = Preference{p.WireGuard, "auto"}
	case "server":
		found := value == "auto"
		for _, n := range nodes {
			if n.Name == value {
				found = true
			}
		}
		if !found {
			return fmt.Errorf("server unavailable for this account and IPv6 requirement")
		}
		p.Server = value
	default:
		return fmt.Errorf("invalid preference")
	}
	if s[id] == nil {
		s[id] = map[string]Preference{}
	}
	s[id][mode] = p
	return nil
}
func load(path string) (Store, error) {
	s := Store{}
	b, e := os.ReadFile(path)
	if os.IsNotExist(e) {
		return s, nil
	}
	if e != nil {
		return nil, e
	}
	e = json.Unmarshal(b, &s)
	if e == nil && s == nil {
		s = Store{}
	}
	return s, e
}
func save(path string, s Store) error {
	b, e := json.Marshal(s)
	if e != nil {
		return e
	}
	if e = os.MkdirAll(filepath.Dir(path), 0700); e != nil {
		return e
	}
	temp := path + ".new"
	if e = os.WriteFile(temp, b, 0600); e != nil {
		return e
	}
	if e = os.Chmod(temp, 0600); e != nil {
		return e
	}
	return os.Rename(temp, path)
}

// Saving a preference does not change routing; the transaction controller
// must apply and verify it before reporting success.
func main() {
	if len(os.Args) == 2 && os.Args[1] == "network-id" {
		var n Network
		if e := json.NewDecoder(io.LimitReader(os.Stdin, 4096)).Decode(&n); e != nil {
			fmt.Fprintln(os.Stderr, "invalid network identity")
			os.Exit(1)
		}
		fmt.Println(networkID(n))
		return
	}
	if len(os.Args) > 1 && os.Args[1] == "observe" {
		if e := observeCLI(os.Args[2:]); e != nil {
			fmt.Fprintln(os.Stderr, e)
			os.Exit(1)
		}
		return
	}
	if len(os.Args) == 2 && os.Args[1] == "country-service" {
		if e := serveCountry(countrySocket); e != nil {
			fmt.Fprintln(os.Stderr, e)
			os.Exit(1)
		}
		return
	}
	if len(os.Args) == 2 && os.Args[1] == "country-get" {
		fmt.Println(readCountry())
		return
	}
	if len(os.Args) < 4 {
		fmt.Fprintln(os.Stderr, "usage: helper catalog.json network.json preferences.json [mode field value]")
		os.Exit(2)
	}
	fail := func(e error) { fmt.Fprintln(os.Stderr, e); os.Exit(1) }
	var nodes []Node
	var network Network
	b, e := os.ReadFile(os.Args[1])
	if e != nil {
		fail(e)
	}
	if e = json.Unmarshal(b, &nodes); e != nil {
		fail(e)
	}
	b, e = os.ReadFile(os.Args[2])
	if e != nil {
		fail(e)
	}
	if e = json.Unmarshal(b, &network); e != nil {
		fail(e)
	}
	nodes = candidates(nodes)
	id := networkID(network)
	applyLatency(nodes, id, "/tmp/proton-latency.json", time.Now().Unix())
	s, e := load(os.Args[3])
	if e != nil {
		fail(e)
	}
	if len(os.Args) == 7 {
		if e = set(s, id, os.Args[4], os.Args[5], os.Args[6], nodes); e != nil {
			fail(e)
		}
		if e = save(os.Args[3], s); e != nil {
			fail(e)
		}
	} else if len(os.Args) != 4 {
		os.Exit(2)
	}
	applyObservations(nodes, id, historyPath(), time.Now().Unix())
	autoCandidates := deprioritizeFailures(automaticCandidates(nodes, readCountry()), nodes)
	autoServer := ""
	if len(autoCandidates) > 0 {
		autoServer = autoCandidates[0]
	}
	result := map[string]any{"network_id": id, "nodes": nodes, "auto_candidates": autoCandidates, "auto_server": autoServer, "local": preference(s, id, "local"), "home": preference(s, id, "home"), "routing_applied": false}
	json.NewEncoder(os.Stdout).Encode(result)
}
