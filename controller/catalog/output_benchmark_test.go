package main

import (
	"encoding/json"
	"fmt"
	"io"
	"testing"
)

func BenchmarkOutput10000(b *testing.B) {
	all := make([]Logical, 10000)
	for i := range all {
		all[i] = Logical{Name: fmt.Sprintf("US-FREE#%d", i), Domain: "node.example", EntryCountry: "US", ExitCountry: "US", Status: 1, Tier: 0, Load: i % 100, Score: float64(i % 91), Features: 16, Servers: []Physical{{EntryIP: "192.0.2.10", ExitIP: "192.0.2.10", Domain: "node.example", Status: 1, X25519PublicKey: "synthetic"}}}
	}
	b.ReportAllocs()
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		rows, e := outputServers(all, options{free: true})
		if e != nil {
			b.Fatal(e)
		}
		enc := json.NewEncoder(io.Discard)
		enc.SetIndent("", "  ")
		if e = enc.Encode(rows); e != nil {
			b.Fatal(e)
		}
	}
}
