//go:build linux

package main

import (
	"bufio"
	"fmt"
	"net"
	"os"
	"strings"
	"testing"
	"time"
)

func TestRealCountrySocketPublication(t *testing.T) {
	if os.Getenv("PROTON_SOCKET_TEST") != "1" {
		t.Skip("router socket opt-in required")
	}
	get := func() string {
		conn, e := net.DialTimeout("unix", "/var/run/proton-country.sock", time.Second)
		if e != nil {
			t.Fatal("country socket unavailable")
		}
		defer conn.Close()
		conn.SetDeadline(time.Now().Add(time.Second))
		fmt.Fprintln(conn, "get")
		s, e := bufio.NewReader(conn).ReadString('\n')
		if e != nil {
			t.Fatal("country socket response unavailable")
		}
		return strings.TrimSpace(s)
	}
	before := get()
	if len(before) != 2 {
		t.Fatal("country cache not initialized")
	}
	publishCountry(before)
	if get() != before {
		t.Fatal("country IPC value changed")
	}
	t.Log("existing Unix socket accepted same-country publication and get; no service reset")
}
