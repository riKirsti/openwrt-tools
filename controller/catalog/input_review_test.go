package main

import (
	"os"
	"testing"
)

func TestReviewCompatibleInputNormalization(t *testing.T) {
	o, e := parseOptions([]string{"--username", " synthetic@proton.me ", "--countries", "us, NL", "--dns", "1.1.1.1, 9.9.9.9", "--allowed-ips", "0.0.0.0/0, ::/0"})
	if e != nil {
		t.Fatal(e)
	}
	if o.username != "synthetic" || o.countries != "US,NL" || o.dns != "1.1.1.1,9.9.9.9" || o.allowed != "0.0.0.0/0,::/0" {
		t.Fatalf("normalization mismatch: %+v", o)
	}
}

func TestReviewHelpSucceedsBeforeSessionInitialization(t *testing.T) {
	t.Setenv("PROTON_SESSION_DIR", t.TempDir())
	old := os.Args
	os.Args = []string{"protonvpn-wg-confgen", "--help"}
	defer func() { os.Args = old }()
	if e := run(); e != nil {
		t.Fatal(e)
	}
	if _, e := os.Stat(sessionPath()); !os.IsNotExist(e) {
		t.Fatal("help created a session")
	}
}
