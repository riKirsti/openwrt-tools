package main

import (
	"reflect"
	"testing"
)

func TestAutomaticCountryPartition(t *testing.T) {
	nodes := []Node{{Name: "CA1", Country: "CA"}, {Name: "US2", Country: "US"}, {Name: "JP3", Country: "JP"}, {Name: "US4", Country: "US"}}
	original := append([]Node(nil), nodes...)
	for _, tc := range []struct {
		country string
		want    []string
	}{
		{"US", []string{"US2", "US4", "CA1", "JP3"}},
		{" us ", []string{"US2", "US4", "CA1", "JP3"}},
		{"", []string{"CA1", "US2", "JP3", "US4"}},
		{"FR", []string{"CA1", "US2", "JP3", "US4"}},
		{"invalid", []string{"CA1", "US2", "JP3", "US4"}},
	} {
		if got := automaticCandidates(nodes, tc.country); !reflect.DeepEqual(got, tc.want) {
			t.Fatalf("%q: %v", tc.country, got)
		}
	}
	if !reflect.DeepEqual(nodes, original) {
		t.Fatal("automatic selection mutated global star ranking")
	}
	if len(automaticCandidates(nil, "US")) != 0 {
		t.Fatal("empty catalog")
	}
}
