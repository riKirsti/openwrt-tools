package main

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

// The real API rejects a present but empty netzone with HTTP 422 / Code 86071.
func TestCachedSessionVerificationOmitsUnknownNetzone(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if values, present := r.Header[http.CanonicalHeaderKey("X-PM-netzone")]; present && (len(values) == 0 || values[0] == "") {
			w.WriteHeader(422)
			_, _ = w.Write([]byte(`{"Code":86071}`))
			return
		}
		_, _ = w.Write([]byte(`{"Code":1000}`))
	}))
	defer server.Close()
	h, e := newHTTP(time.Second)
	if e != nil {
		t.Fatal(e)
	}
	defer h.CloseIdleConnections()
	c := Client{http: h, base: server.URL, session: &Session{UID: "fixture", AccessToken: "fixture"}}
	if e := c.verify(context.Background(), false); e != nil {
		t.Fatalf("cached session verification rejected: %v", e)
	}
	c.netzone = "192.0.2.0"
	req, e := c.request(context.Background(), "GET", "/vpn/v1/logicals", nil)
	if e != nil || req.Header.Get("X-PM-netzone") != "192.0.2.0" {
		t.Fatal("known netzone must still be supplied")
	}
}
