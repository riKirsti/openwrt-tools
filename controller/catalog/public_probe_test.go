package main

import (
	"encoding/json"
	"io"
	"net/http"
	"os"
	"testing"
	"time"
)

func TestLabPublicReadOnly(t *testing.T) {
	if os.Getenv("LAB_PUBLIC_PROBE") != "1" {
		t.Skip("explicit read-only public probe disabled")
	}
	client, err := newHTTP(8 * time.Second)
	if err != nil {
		t.Fatal(err)
	}
	defer client.CloseIdleConnections()
	for _, path := range []string{"/tests/ping", "/vpn/v1/location"} {
		req, err := http.NewRequest("GET", "https://vpn-api.proton.me"+path, nil)
		if err != nil {
			t.Fatal(err)
		}
		req.Header.Set("x-pm-appversion", "linux-vpn-gui@5.8.7+x86-64")
		req.Header.Set("User-Agent", "ProtonVPN/5.8.7 (Linux; ubuntu/24.04)")
		resp, err := client.Do(req)
		if err != nil {
			t.Logf("PUBLIC_PROBE path=%s transport_error=%T", path, err)
			continue
		}
		raw, _ := io.ReadAll(io.LimitReader(resp.Body, 65536))
		resp.Body.Close()
		var body struct{ Code int }
		json.Unmarshal(raw, &body)
		t.Logf("PUBLIC_PROBE path=%s http=%d api_code=%d", path, resp.StatusCode, body.Code)
	}
}
