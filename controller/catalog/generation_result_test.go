package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"strings"
	"testing"
)

type rejectedWriter struct{}

func (rejectedWriter) Write([]byte) (int, error) { return 0, errors.New("write rejected") }
func TestGenerationMetadataContract(t *testing.T) {
	var b bytes.Buffer
	if err := generationResult(&b, true, "session", "12345", 2000000000, "US-FREE#155", "1.2.3.4:51820"); err != nil {
		t.Fatal(err)
	}
	var obj map[string]any
	if json.Unmarshal(b.Bytes(), &obj) != nil || obj["version"] != float64(1) || obj["serial"] != "12345" || obj["server"] != "US-FREE#155" || len(obj) != 6 {
		t.Fatal("metadata contract differs")
	}
	for key := range obj {
		if strings.Contains(strings.ToLower(key), "key") {
			t.Fatal("key in metadata")
		}
	}
	b.Reset()
	if err := generationResult(&b, false, "session", "12345", 2000000000, "", ""); err != nil || !strings.HasPrefix(b.String(), "Certificate: session, serial 12345, expires ") {
		t.Fatal("human CLI compatibility")
	}
	if generationResult(rejectedWriter{}, true, "session", "1", 2000000000, "", "") == nil {
		t.Fatal("ignored result write failure")
	}
}
func TestMetadataFlag(t *testing.T) {
	o, e := parseOptions([]string{"--metadata-json", "--server", "US-FREE#155"})
	if e != nil || !o.metadataJSON {
		t.Fatal("metadata option", e)
	}
}
