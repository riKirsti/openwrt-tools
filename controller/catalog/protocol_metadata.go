package main

import "context"

// Optional enrichment preserves v1 ranking/load data and legacy compatibility.
func (c *Client) withProtocolEntries(ctx context.Context, all []Logical) []Logical {
	var result struct {
		apiStatus
		LogicalServers []Logical
	}
	if c.call(ctx, "GET", "/vpn/v2/logicals?WithEntriesForProtocols=WireGuardUDP%2CWireGuardTCP%2CWireGuardTLS&WithState=true", nil, &result, true) != nil {
		return all
	}
	entries := map[string]map[string]ProtocolEntry{}
	for _, n := range result.LogicalServers {
		for _, p := range n.Servers {
			if p.ID != "" && p.EntryPerProtocol != nil {
				entries[p.ID] = p.EntryPerProtocol
			}
		}
	}
	for i := range all {
		for j := range all[i].Servers {
			p := &all[i].Servers[j]
			if e, ok := entries[p.ID]; ok {
				p.EntryPerProtocol = e
			}
		}
	}
	return all
}
