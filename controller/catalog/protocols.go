package main

import (
	"net"
	"strconv"
	"strings"
)

type ProtocolEntry struct {
	IPv4  string `json:"IPv4"`
	Ports *[]int `json:"Ports,omitempty"`
}

var protocolNames = []string{"wireguard-udp", "wireguard-tcp", "stealth"}

func protocolName(s string) string {
	if s == "" {
		return "wireguard-udp"
	}
	return s
}
func protocolAPI(s string) string {
	switch protocolName(s) {
	case "wireguard-udp":
		return "WireGuardUDP"
	case "wireguard-tcp":
		return "WireGuardTCP"
	case "stealth":
		return "WireGuardTLS"
	}
	return ""
}
func resolveProtocol(ip string, entries map[string]ProtocolEntry, protocol string) (string, []int, bool, bool) {
	name := protocolAPI(protocol)
	if name == "" {
		return "", nil, false, false
	}
	explicit := false
	var ports []int
	if entry, ok := entries[name]; ok {
		if entry.IPv4 != "" {
			ip = entry.IPv4
			explicit = true
		}
		if entry.Ports != nil {
			if len(*entry.Ports) == 0 || len(*entry.Ports) > 16 {
				return "", nil, false, false
			}
			seen := map[int]bool{}
			for _, p := range *entry.Ports {
				if p < 1 || p > 65535 {
					return "", nil, false, false
				}
				if !seen[p] {
					ports = append(ports, p)
					seen[p] = true
				}
			}
		}
	}
	parsed := net.ParseIP(ip)
	if parsed == nil || parsed.IsUnspecified() || parsed.IsMulticast() {
		return "", nil, false, false
	}
	if protocolName(protocol) != "wireguard-udp" && parsed.To4() == nil {
		return "", nil, false, false
	}
	return parsed.String(), ports, explicit, true
}
func protocolPortsComment(ports []int) string {
	if ports == nil {
		return ""
	}
	s := make([]string, len(ports))
	for i, p := range ports {
		s[i] = strconv.Itoa(p)
	}
	return "# TransportPorts = " + strings.Join(s, ",") + "\n"
}
