package main

import (
	"context"
	"encoding/json"
	"io"
	"os"
	"strings"
	"testing"
	"time"
)

// Explicit read-only real API diagnosis. Never refreshes or modifies authorization.
func TestRealHeaderDiagnosis(t *testing.T) {
	if os.Getenv("PROTON_ROUTER_TEST") != "1" {
		t.Skip("real API opt-in required")
	}
	raw, e := os.ReadFile(sessionPath())
	if e != nil {
		t.Fatal("isolated authorization missing")
	}
	var saved Saved
	if json.Unmarshal(raw, &saved) != nil {
		t.Fatal("invalid isolated authorization")
	}
	h, e := newHTTP(10 * time.Second)
	if e != nil {
		t.Fatal(e)
	}
	defer h.CloseIdleConnections()
	c := Client{http: h, base: "https://vpn-api.proton.me", session: saved.Session}
	for _, variant := range []string{"candidate-current", "omit-empty-netzone", "valid-netzone", "omit-netzone-and-ims"} {
		req, e := c.request(context.Background(), "GET", "/vpn/v1/logicals", nil)
		if e != nil {
			t.Fatal(e)
		}
		switch variant {
		case "omit-empty-netzone":
			req.Header.Del("X-PM-netzone")
		case "valid-netzone":
			req.Header.Set("X-PM-netzone", "0.0.0.0")
		case "omit-netzone-and-ims":
			req.Header.Del("X-PM-netzone")
			req.Header.Del("If-Modified-Since")
		}
		resp, e := h.Do(req)
		if e != nil {
			t.Logf("DIAG variant=%s network_error=true", variant)
			continue
		}
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 32*1024*1024))
		resp.Body.Close()
		var data struct {
			Code  int
			Error string
		}
		json.Unmarshal(b, &data)
		keywords := []string{}
		for _, s := range []string{"netzone", "header", "parameter", "invalid", "version", "token"} {
			if strings.Contains(strings.ToLower(data.Error), s) {
				keywords = append(keywords, s)
			}
		}
		result := map[string]any{"variant": variant, "http": resp.StatusCode, "code": data.Code, "error_keywords": keywords}
		out, _ := json.Marshal(result)
		t.Log("DIAG " + string(out))
	}
}
