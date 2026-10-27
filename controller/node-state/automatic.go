package main

import (
	"bufio"
	"fmt"
	"net"
	"os"
	"strings"
	"time"
)

const countrySocket = "/var/run/proton-country.sock"

func countryCode(s string) string {
	s = strings.ToUpper(strings.TrimSpace(s))
	if len(s) != 2 || s[0] < 'A' || s[0] > 'Z' || s[1] < 'A' || s[1] > 'Z' {
		return ""
	}
	return s
}

// The IPC socket carries a country code; the value lives only in this process.
// No address, coordinates, account data or country value is written to disk.
func serveCountry(path string) error {
	_ = os.Remove(path)
	l, err := net.Listen("unix", path)
	if err != nil {
		return err
	}
	defer l.Close()
	if err = os.Chmod(path, 0600); err != nil {
		return err
	}
	return serveCountryListener(l)
}

func serveCountryListener(l net.Listener) error {
	country := ""
	for {
		c, err := l.Accept()
		if err != nil {
			return err
		}
		_ = c.SetDeadline(time.Now().Add(time.Second))
		line, _ := bufio.NewReaderSize(c, 64).ReadString('\n')
		fields := strings.Fields(line)
		if len(fields) == 2 && fields[0] == "set" && country == "" {
			country = countryCode(fields[1])
		}
		fmt.Fprintln(c, country)
		c.Close()
	}
}

func readCountry() string {
	c, e := net.DialTimeout("unix", countrySocket, 200*time.Millisecond)
	if e != nil {
		return ""
	}
	defer c.Close()
	_ = c.SetDeadline(time.Now().Add(200 * time.Millisecond))
	fmt.Fprintln(c, "get")
	line, _ := bufio.NewReader(c).ReadString('\n')
	return countryCode(line)
}

// Stable partition only: global ranks and their star ratings are untouched.
func automaticCandidates(nodes []Node, country string) []string {
	country = countryCode(country)
	names := make([]string, 0, len(nodes))
	if country != "" {
		for _, n := range nodes {
			if n.Country == country {
				names = append(names, n.Name)
			}
		}
	}
	for _, n := range nodes {
		if country == "" || n.Country != country {
			names = append(names, n.Name)
		}
	}
	return names
}
