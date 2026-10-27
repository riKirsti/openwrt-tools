package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func f(v float64) *float64 { return &v }
func TestStagedRanking(t *testing.T) {
	nodes := []Node{{Name: "overloaded", Score: 0, Load: 91, LatencyMS: f(20)}, {Name: "boundary", Score: 1, Load: 90, LatencyMS: f(30)}, {Name: "fast", Score: 2, Load: 20, LatencyMS: f(20)}, {Name: "slower", Score: 0, LatencyMS: f(30.001)}, {Name: "far", Score: 0, LatencyMS: f(200)}, {Name: "unknown", Score: 0}}
	rankRecommendations(nodes)
	want := []string{"boundary", "fast", "overloaded", "slower", "far", "unknown"}
	for i, n := range nodes {
		if n.Name != want[i] {
			t.Fatalf("rank %d: %s", i, n.Name)
		}
		if i < 5 && (n.RecommendationRank == nil || *n.RecommendationRank != i) {
			t.Fatal("incorrect rank")
		}
	}
	if nodes[5].RecommendationRank != nil {
		t.Fatal("unknown ranked")
	}
}
func TestQuarterWindowAndTies(t *testing.T) {
	nodes := []Node{{Name: "minimum", Score: 9, Load: 10, LatencyMS: f(100)}, {Name: "outside", Score: 0, LatencyMS: f(125.001)}, {Name: "boundary", Score: 1, Load: 20, LatencyMS: f(125)}, {Name: "load", Score: 1, Load: 10, LatencyMS: f(124)}, {Name: "rtt", Score: 1, Load: 10, LatencyMS: f(123)}}
	rankRecommendations(nodes)
	want := []string{"rtt", "load", "boundary", "minimum", "outside"}
	for i, n := range nodes {
		if n.Name != want[i] {
			t.Fatalf("rank %d: %s", i, n.Name)
		}
	}
}
func TestCacheValidity(t *testing.T) {
	p := filepath.Join(t.TempDir(), "cache.json")
	b, _ := json.Marshal(LatencyCache{"network", 1000, map[string]LatencyResult{"1.1.1.1": {f(20), "icmp", "ok"}}})
	os.WriteFile(p, b, 0600)
	for _, tt := range []struct {
		id    string
		now   int64
		valid bool
	}{{"network", 1100, true}, {"other", 1100, false}, {"network", 1601, false}, {"network", 999, false}} {
		nodes := []Node{{Name: "test", Score: 1, Endpoints: []Endpoint{{EntryIP: "1.1.1.1"}}}}
		applyLatency(nodes, tt.id, p, tt.now)
		if (nodes[0].RecommendationRank != nil) != tt.valid {
			t.Fatal("cache validation failed")
		}
	}
}
