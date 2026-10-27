package main

import (
	"context"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	srp "github.com/ProtonMail/go-srp"
)

func fixtureClient(t *testing.T, h http.HandlerFunc) *Client {
	t.Helper()
	s := httptest.NewServer(h)
	t.Cleanup(s.Close)
	c, e := newHTTP(time.Second)
	if e != nil {
		t.Fatal(e)
	}
	t.Cleanup(c.CloseIdleConnections)
	return &Client{http: c, base: s.URL}
}
func TestSRPAndTOTP(t *testing.T) {
	data, e := os.ReadFile("testdata/auth-info.json")
	if e != nil {
		t.Fatal(e)
	}
	var info struct{ Modulus, Salt string }
	if e = json.Unmarshal(data, &info); e != nil {
		t.Fatal(e)
	}
	salt, _ := base64.StdEncoding.DecodeString(info.Salt)
	a, e := srp.NewAuthForVerifier([]byte("local-fixture-password"), info.Modulus, salt)
	if e != nil {
		t.Fatal(e)
	}
	verifier, e := a.GenerateVerifier(2048)
	if e != nil {
		t.Fatal(e)
	}
	for _, tc := range []struct {
		name          string
		badProof, two bool
	}{{"normal", false, false}, {"totp", false, true}, {"reject-bad-server-proof", true, false}} {
		t.Run(tc.name, func(t *testing.T) {
			server, e := srp.NewServerFromSigned(info.Modulus, verifier, 2048)
			if e != nil {
				t.Fatal(e)
			}
			challenge, e := server.GenerateChallenge()
			if e != nil {
				t.Fatal(e)
			}
			c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
				w.Header().Set("Content-Type", "application/json")
				switch r.URL.Path {
				case "/auth/info":
					json.NewEncoder(w).Encode(map[string]any{"Code": 1000, "Version": 4, "Modulus": info.Modulus, "Salt": info.Salt, "ServerEphemeral": base64.StdEncoding.EncodeToString(challenge), "SRPSession": "synthetic"})
				case "/auth":
					var b map[string]string
					json.NewDecoder(r.Body).Decode(&b)
					ce, _ := base64.StdEncoding.DecodeString(b["ClientEphemeral"])
					cp, _ := base64.StdEncoding.DecodeString(b["ClientProof"])
					proof, e := server.VerifyProofs(ce, cp)
					if e != nil {
						t.Error(e)
						w.WriteHeader(422)
						fmt.Fprint(w, `{"Code":8002}`)
						return
					}
					if tc.badProof {
						proof[0] ^= 1
					}
					scopes := []string{"vpn"}
					if tc.two {
						scopes = []string{"twofactor"}
					}
					json.NewEncoder(w).Encode(map[string]any{"Code": 1000, "UID": "fixture-uid", "AccessToken": "fixture-access", "RefreshToken": "fixture-refresh", "ExpiresIn": 86400, "ServerProof": base64.StdEncoding.EncodeToString(proof), "Scopes": scopes})
				case "/auth/2fa":
					var b map[string]string
					json.NewDecoder(r.Body).Decode(&b)
					if b["TwoFactorCode"] != "123456" {
						t.Error("incorrect test TOTP")
					}
					fmt.Fprint(w, `{"Code":1000,"Scopes":["vpn"]}`)
				default:
					t.Errorf("unexpected request %s", r.URL.Path)
				}
			})
			persisted := false
			c.persist = func(s *Session) error { persisted = true; return nil }
			password := []byte("local-fixture-password")
			e = c.login(context.Background(), "fixture", password, "123456")
			if tc.badProof {
				if e == nil || persisted {
					t.Fatal("bad proof accepted or persisted")
				}
			} else if e != nil || !persisted {
				t.Fatalf("login failed: %v", e)
			}
			for _, b := range password {
				if b != 0 {
					t.Fatal("password buffer not cleared")
				}
			}
		})
	}
}
func TestRefreshOn401Once(t *testing.T) {
	refreshes, requests := 0, 0
	c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/auth/refresh" {
			refreshes++
			fmt.Fprint(w, `{"Code":1000,"AccessToken":"new","RefreshToken":"new-refresh","ExpiresIn":86400}`)
			return
		}
		requests++
		if r.Header.Get("Authorization") == "Bearer old" {
			w.WriteHeader(401)
			fmt.Fprint(w, `{"Code":401}`)
		} else {
			fmt.Fprint(w, `{"Code":1000}`)
		}
	})
	c.session = &Session{UID: "id", AccessToken: "old", RefreshToken: "refresh"}
	persisted := false
	c.persist = func(s *Session) error { persisted = s.AccessToken == "new" && s.UID == "id"; return nil }
	if e := c.call(context.Background(), "GET", "/data", nil, nil, true); e != nil {
		t.Fatal(e)
	}
	if requests != 2 || refreshes != 1 || !persisted {
		t.Fatal("wrong refresh sequence")
	}
}
func TestNoRetryStormOrSecretEcho(t *testing.T) {
	for _, status := range []int{401, 429, 503} {
		t.Run(fmt.Sprint(status), func(t *testing.T) {
			n := 0
			c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
				n++
				w.WriteHeader(status)
				fmt.Fprint(w, `{"Code":9001,"Error":"secret-fixture-token"}`)
			})
			e := c.call(context.Background(), "GET", "/data", nil, nil, false)
			if e == nil || strings.Contains(e.Error(), "secret-fixture-token") || n != 1 {
				t.Fatal("unsafe error or retries")
			}
		})
	}
}
func TestMalformedAndTimeout(t *testing.T) {
	c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) { fmt.Fprint(w, "secret-fixture-token NOT JSON") })
	if e := c.call(context.Background(), "GET", "/data", nil, nil, false); e == nil || strings.Contains(e.Error(), "secret-fixture-token") {
		t.Fatal("unsafe malformed response")
	}
	c = fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
		time.Sleep(50 * time.Millisecond)
		fmt.Fprint(w, `{"Code":1000}`)
	})
	c.http.Timeout = 5 * time.Millisecond
	if e := c.call(context.Background(), "GET", "/data", nil, nil, false); e == nil {
		t.Fatal("timeout ignored")
	}
}
func TestTLSVerification(t *testing.T) {
	s := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { fmt.Fprint(w, `{"Code":1000}`) }))
	defer s.Close()
	h, e := newHTTP(time.Second)
	if e != nil {
		t.Fatal(e)
	}
	defer h.CloseIdleConnections()
	c := &Client{http: h, base: s.URL}
	if e = c.call(context.Background(), "GET", "/", nil, nil, false); e == nil {
		t.Fatal("untrusted certificate accepted")
	}
	roots := x509.NewCertPool()
	roots.AddCert(s.Certificate())
	h.Transport.(*http.Transport).TLSClientConfig.RootCAs = roots
	if e = c.call(context.Background(), "GET", "/", nil, nil, false); e != nil {
		t.Fatal(e)
	}
}
func TestSessionRoundtripAndIdentity(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("PROTON_SESSION_DIR", dir)
	s := &Session{UID: "fixture", AccessToken: "first", RefreshToken: "refresh", ExpiresIn: 86400}
	if e := saveSession("fixture", s); e != nil {
		t.Fatal(e)
	}
	s.AccessToken = "second"
	if e := saveSession("fixture", s); e != nil {
		t.Fatal(e)
	}
	saved, e := loadSession("fixture")
	if e != nil || saved.Session.AccessToken != "second" {
		t.Fatal("save/reload failure", e)
	}
	if _, e = loadSession("different"); e == nil {
		t.Fatal("identity mismatch accepted")
	}
	if e = safeWrite(dir, []byte("bad")); e == nil {
		t.Fatal("directory write accepted")
	}
	if e = os.WriteFile(filepath.Join(dir, ".protonvpn-session.json"), []byte("broken"), 0600); e != nil {
		t.Fatal(e)
	}
	if _, e = loadSession("fixture"); e == nil {
		t.Fatal("corrupt session accepted")
	}
}
func TestRevokeCount(t *testing.T) {
	for _, count := range []int{0, 1} {
		c := fixtureClient(t, func(w http.ResponseWriter, r *http.Request) {
			var b map[string]string
			json.NewDecoder(r.Body).Decode(&b)
			if r.Method != "DELETE" || b["SerialNumber"] != "owned" {
				t.Error("wrong revocation request")
			}
			fmt.Fprintf(w, `{"Code":1000,"Count":%d}`, count)
		})
		if e := c.revoke(context.Background(), "owned"); (e == nil) != (count == 1) {
			t.Fatal("wrong revocation outcome")
		}
	}
}
func TestInvalidMarkAndDNS(t *testing.T) {
	t.Setenv("PROTON_WAN_MARK", "0")
	if _, e := newHTTP(time.Second); e == nil {
		t.Fatal("zero mark accepted")
	}
	t.Setenv("PROTON_WAN_MARK", "")
	t.Setenv("PROTON_WAN_DNS", "hostname.invalid")
	if _, e := newHTTP(time.Second); e == nil {
		t.Fatal("non-IP bootstrap accepted")
	}
}
