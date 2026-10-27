package main

import (
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
)

func TestReviewLogoutRequiresConfirmedRevocation(t *testing.T) {
	for _, tc := range []struct {
		name                                    string
		firstStatus, refreshStatus, retryStatus int
		wantSuccess                             bool
		wantRequests                            string
	}{
		{"valid-access", 200, 0, 0, true, "DELETE"},
		{"expired-access-refreshed", 401, 200, 200, true, "DELETE POST DELETE"},
		{"failed-refresh-retains-session", 401, 503, 0, false, "DELETE POST"},
		{"second-401-retains-refreshed-session", 401, 200, 401, false, "DELETE POST DELETE"},
		{"server-error-retains-session", 503, 0, 0, false, "DELETE"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			t.Setenv("PROTON_SESSION_DIR", t.TempDir())
			t.Setenv("PROTON_WAN_MARK", "")
			t.Setenv("PROTON_WAN_DNS", "")
			if e := saveSession("synthetic", &Session{UID: "synthetic", AccessToken: "expired", RefreshToken: "refresh", ExpiresIn: 1}); e != nil {
				t.Fatal(e)
			}
			before, _ := os.ReadFile(sessionPath())
			requests := []string{}
			deletes := 0
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				requests = append(requests, r.Method)
				if r.Method == "POST" && r.URL.Path == "/auth/refresh" {
					w.WriteHeader(tc.refreshStatus)
					if tc.refreshStatus == 200 {
						fmt.Fprint(w, `{"Code":1000,"AccessToken":"renewed","RefreshToken":"renewed-refresh","ExpiresIn":86400}`)
					} else {
						fmt.Fprint(w, `{"Code":2000}`)
					}
					return
				}
				if r.Method != "DELETE" || r.URL.Path != "/auth" {
					t.Errorf("unexpected request %s %s", r.Method, r.URL.Path)
					w.WriteHeader(500)
					return
				}
				status := tc.firstStatus
				if deletes > 0 {
					status = tc.retryStatus
				}
				deletes++
				w.WriteHeader(status)
				if status == 200 {
					fmt.Fprint(w, `{"Code":1000}`)
				} else {
					fmt.Fprint(w, `{"Code":2000}`)
				}
			}))
			defer server.Close()
			old := os.Args
			os.Args = []string{"proton-session-revoke", "--api-url", server.URL}
			defer func() { os.Args = old }()
			e := run()
			if (e == nil) != tc.wantSuccess {
				t.Fatalf("success=%v error=%v", tc.wantSuccess, e)
			}
			if strings.Join(requests, " ") != tc.wantRequests {
				t.Fatalf("requests: %v", requests)
			}
			after, readErr := os.ReadFile(sessionPath())
			if tc.wantSuccess {
				if !os.IsNotExist(readErr) {
					t.Fatal("confirmed logout retained cache")
				}
			} else {
				if readErr != nil {
					t.Fatal("unconfirmed logout deleted cache")
				}
				if tc.refreshStatus != 200 && string(after) != string(before) {
					t.Fatal("failed refresh altered cache")
				}
				if tc.refreshStatus == 200 && !strings.Contains(string(after), "renewed-refresh") {
					t.Fatal("retry failed without retaining refreshed authorization")
				}
			}
		})
	}
}
