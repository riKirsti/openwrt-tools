package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestFailedRefreshPreservesCachedAuthorization(t *testing.T) {
	for _, status := range []int{401, 429, 503} {
		t.Run(fmt.Sprint(status), func(t *testing.T) {
			t.Setenv("PROTON_SESSION_DIR", t.TempDir())
			original := &Session{UID: "synthetic-id", AccessToken: "old-access", RefreshToken: "old-refresh", ExpiresIn: 86400}
			if e := saveSession("synthetic", original); e != nil {
				t.Fatal(e)
			}
			before, _ := os.ReadFile(sessionPath())
			calls := 0
			c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
				calls++
				w.WriteHeader(status)
				fmt.Fprint(w, `{"Code":2000}`)
			})
			c.session = original
			c.persist = func(s *Session) error { return saveSession("synthetic", s) }
			if e := c.refresh(context.Background()); e == nil {
				t.Fatal("failed refresh accepted")
			}
			after, _ := os.ReadFile(sessionPath())
			if string(before) != string(after) || c.session != original || calls != 1 {
				t.Fatal("failed refresh changed authorization or retried")
			}
		})
	}
}

func TestGenerationFailureRevokesOnlyItsOwnCertificate(t *testing.T) {
	for _, cleanupFails := range []bool{false, true} {
		t.Run(fmt.Sprint(cleanupFails), func(t *testing.T) {
			created, deleted := 0, 0
			c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
				switch {
				case r.URL.Path == "/vpn/v1/location":
					fmt.Fprint(w, `{"Code":1000,"IP":"192.0.2.1","Country":"US"}`)
				case strings.HasPrefix(r.URL.Path, "/vpn/v1/logicals"):
					fmt.Fprint(w, `{"Code":1000,"LogicalServers":[{"Name":"US-FREE#1","Status":1,"Tier":0,"Servers":[{"Status":1,"EntryIP":"192.0.2.2","X25519PublicKey":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}]}]}`)
				case r.Method == "POST":
					created++
					json.NewEncoder(w).Encode(map[string]any{"Code": 1000, "SerialNumber": "987654321", "ExpirationTime": time.Now().Add(time.Hour).Unix()})
				case r.Method == "DELETE":
					deleted++
					var b map[string]string
					json.NewDecoder(r.Body).Decode(&b)
					if b["SerialNumber"] != "987654321" {
						t.Error("revoked unrelated credential")
					}
					if cleanupFails {
						w.WriteHeader(503)
						fmt.Fprint(w, `{"Code":2000}`)
					} else {
						fmt.Fprint(w, `{"Code":1000,"Count":1}`)
					}
				default:
					t.Errorf("unexpected request %s %s", r.Method, r.URL.Path)
				}
			})
			dir := t.TempDir()
			output := filepath.Join(dir, "missing-parent", "config")
			e := generate(c, context.Background(), options{server: "US-FREE#1", duration: "10m", noSave: true, output: output})
			if e == nil || created != 1 || deleted != 1 {
				t.Fatal("write failure not cleaned exactly once", e)
			}
			if cleanupFails && !strings.Contains(e.Error(), "pending cleanup serial 987654321") {
				t.Fatal("cleanup ownership lost")
			}
			if _, e = os.Stat(output); !os.IsNotExist(e) {
				t.Fatal("unexpected output")
			}
		})
	}
}

func TestRefreshPersistenceFailureIsReported(t *testing.T) {
	c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprint(w, `{"Code":1000,"AccessToken":"new","RefreshToken":"new-refresh","ExpiresIn":86400}`)
	})
	c.session = &Session{UID: "synthetic-id", AccessToken: "old", RefreshToken: "old-refresh"}
	c.persist = func(*Session) error { return errors.New("synthetic disk failure") }
	if e := c.refresh(context.Background()); e == nil {
		t.Fatal("persistence failure hidden")
	}
}

func TestInvalidRefreshResponseNotPersisted(t *testing.T) {
	c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) { fmt.Fprint(w, `{"Code":1000,"AccessToken":"new"}`) })
	c.session = &Session{UID: "synthetic-id", AccessToken: "old", RefreshToken: "old-refresh"}
	persisted := false
	c.persist = func(*Session) error { persisted = true; return nil }
	if e := c.refresh(context.Background()); e == nil || persisted {
		t.Fatal("incomplete authorization accepted")
	}
}
