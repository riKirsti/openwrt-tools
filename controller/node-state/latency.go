package main

import (
	"encoding/json"
	"math"
	"os"
	"sort"
)

type LatencyResult struct {
	MS     *float64 `json:"ms"`
	Method string   `json:"method"`
	Status string   `json:"status"`
}
type LatencyCache struct {
	NetworkID string                   `json:"network_id"`
	Checked   int64                    `json:"checked"`
	Results   map[string]LatencyResult `json:"results"`
}

func validNumber(v float64) bool { return !math.IsNaN(v) && !math.IsInf(v, 0) }
func applyLatency(nodes []Node, id, path string, now int64) {
	b, e := os.ReadFile(path)
	if e != nil {
		return
	}
	var cache LatencyCache
	if json.Unmarshal(b, &cache) != nil || cache.NetworkID != id || now-cache.Checked > 600 || cache.Checked > now {
		return
	}
	for i := range nodes {
		n := &nodes[i]
		// Measure the first endpoint: this is the endpoint selected by the current
		// WireGuard config generator. Do not substitute a different server address.
		if len(n.Endpoints) == 0 {
			continue
		}
		r, ok := cache.Results[n.Endpoints[0].EntryIP]
		if ok && r.Status == "ok" && r.MS != nil && validNumber(*r.MS) && *r.MS > 0 {
			v := *r.MS
			n.LatencyMS = &v
			n.LatencyMethod = r.Method
		}
	}
	rankRecommendations(nodes)
}

// rankRecommendations keeps the fast window ahead of slower measured servers.
// Within that window, load <=90% is preferred, then Proton Score, load and RTT.
func rankRecommendations(nodes []Node) {
	measured := func(n Node) bool {
		return n.LatencyMS != nil && validNumber(*n.LatencyMS) && *n.LatencyMS > 0 && validNumber(n.Score)
	}
	minimum := math.Inf(1)
	for i := range nodes {
		nodes[i].RecommendationRank = nil
		if measured(nodes[i]) {
			minimum = math.Min(minimum, *nodes[i].LatencyMS)
		}
	}
	threshold := minimum + math.Max(10, minimum*0.25)
	group := func(n Node) int {
		if !measured(n) {
			return 3
		}
		if *n.LatencyMS > threshold {
			return 2
		}
		if n.Load > 90 {
			return 1
		}
		return 0
	}
	sort.SliceStable(nodes, func(i, j int) bool {
		a, b := nodes[i], nodes[j]
		ga, gb := group(a), group(b)
		if ga != gb {
			return ga < gb
		}
		if ga == 2 && *a.LatencyMS != *b.LatencyMS {
			return *a.LatencyMS < *b.LatencyMS
		}
		if a.Score != b.Score {
			return a.Score < b.Score
		}
		if a.Load != b.Load {
			return a.Load < b.Load
		}
		if ga < 2 && *a.LatencyMS != *b.LatencyMS {
			return *a.LatencyMS < *b.LatencyMS
		}
		return a.Name < b.Name
	})
	for i := range nodes {
		if measured(nodes[i]) {
			rank := i
			nodes[i].RecommendationRank = &rank
		}
	}
}
