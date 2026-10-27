package main

import (
	"bufio"
	"fmt"
	"net"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestCountryMemory(t *testing.T) {
	path := filepath.Join(t.TempDir(), "country.sock")
	start := func() (net.Listener, chan error) {
		l, e := net.Listen("unix", path)
		if e != nil {
			t.Fatal(e)
		}
		done := make(chan error, 1)
		go func() { done <- serveCountryListener(l) }()
		return l, done
	}
	call := func(command string) string {
		c, e := net.Dial("unix", path)
		if e != nil {
			t.Fatal(e)
		}
		defer c.Close()
		c.SetDeadline(time.Now().Add(time.Second))
		fmt.Fprintln(c, command)
		s, e := bufio.NewReader(c).ReadString('\n')
		if e != nil {
			t.Fatal(e)
		}
		return strings.TrimSpace(s)
	}
	l, done := start()
	if call("get") != "" {
		t.Fatal("new process has country")
	}
	if call("set invalid") != "" {
		t.Fatal("invalid country retained")
	}
	if call("set US") != "US" {
		t.Fatal("country not retained")
	}
	if call("set JP") != "US" {
		t.Fatal("first country overwritten")
	}
	if call("get") != "US" {
		t.Fatal("country lost")
	}
	l.Close()
	<-done
	l, done = start()
	if call("get") != "" {
		t.Fatal("country persisted across service restart")
	}
	l.Close()
	<-done
}
