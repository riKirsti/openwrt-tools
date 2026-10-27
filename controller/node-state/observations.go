package main

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"
)

const failureCooldown = int64(300)
const verificationLifetime = int64(6 * 3600)

type Observation struct {
	Success         int64  `json:"success,omitempty"`
	SuccessEndpoint string `json:"success_endpoint,omitempty"`
	Port            int    `json:"port,omitempty"`
	Failure         int64  `json:"failure,omitempty"`
	FailureEndpoint string `json:"failure_endpoint,omitempty"`
}
type History struct {
	Version int                    `json:"version"`
	Boot    string                 `json:"boot"`
	Records map[string]Observation `json:"records"`
}

func historyPath() string {
	if p := os.Getenv("PROTON_HISTORY_PATH"); filepath.IsAbs(p) {
		return p
	}
	return "/tmp/proton-runtime/server-observations.json"
}
func bootID() string {
	b, _ := os.ReadFile("/proc/sys/kernel/random/boot_id")
	return strings.TrimSpace(string(b))
}
func endpointFingerprint(ip, key string) string {
	return fmt.Sprintf("%x", sha256.Sum256([]byte(ip+"\n"+key)))
}
func historyKey(network, protocol, server string) string {
	return network + ":" + protocol + ":" + server
}
func readHistory(path, boot string) History {
	h := History{Version: 1, Boot: boot, Records: map[string]Observation{}}
	f, e := os.Open(path)
	if e != nil {
		return h
	}
	defer f.Close()
	info, e := f.Stat()
	if e != nil || !info.Mode().IsRegular() || info.Size() > 256*1024 {
		return h
	}
	var saved History
	if json.NewDecoder(f).Decode(&saved) != nil || saved.Version != 1 || saved.Boot != boot || saved.Records == nil || len(saved.Records) > 256 {
		return h
	}
	return saved
}
func writeHistory(path string, h History) error {
	if info, e := os.Lstat(path); e == nil && !info.Mode().IsRegular() {
		return errors.New("non-regular observation destination")
	}
	dir := filepath.Dir(path)
	if e := os.MkdirAll(dir, 0700); e != nil {
		return e
	}
	f, e := os.CreateTemp(dir, ".server-observations-*")
	if e != nil {
		return e
	}
	defer os.Remove(f.Name())
	if e = f.Chmod(0600); e == nil {
		e = json.NewEncoder(f).Encode(h)
	}
	closeErr := f.Close()
	if e != nil {
		return e
	}
	if closeErr != nil {
		return closeErr
	}
	return os.Rename(f.Name(), path)
}
func observeCLI(args []string) error {
	if len(args) != 7 {
		return errors.New("observation requires event, network, protocol, server, address, public key and port")
	}
	event, network, protocol, server, ip, key := args[0], args[1], args[2], args[3], args[4], args[5]
	if event != "success" && event != "failure" {
		return errors.New("invalid observation event")
	}
	decoded, e := hex.DecodeString(network)
	if e != nil || len(decoded) != 32 || protocolAPI(protocol) == "" || !regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9#_-]{0,63}$`).MatchString(server) {
		return errors.New("invalid observation scope")
	}
	pub, e := base64.StdEncoding.DecodeString(key)
	if e != nil || len(pub) != 32 {
		return errors.New("invalid observation public key")
	}
	resolved, _, _, ok := resolveProtocol(ip, nil, protocol)
	if !ok {
		return errors.New("invalid observation address")
	}
	port, e := strconv.Atoi(args[6])
	if e != nil || port < 0 || port > 65535 || (event == "success" && port == 0) {
		return errors.New("invalid observation port")
	}
	path := historyPath()
	if e = os.MkdirAll(filepath.Dir(path), 0700); e != nil {
		return e
	}
	var release func()
	for i := 0; i < 25; i++ {
		release, e = lockHistory(path + ".guard")
		if e == nil {
			break
		}
		time.Sleep(40 * time.Millisecond)
	}
	if e != nil {
		return e
	}
	defer release()
	h := readHistory(path, bootID())
	now := time.Now().Unix()
	k := historyKey(network, protocol, server)
	v := h.Records[k]
	finger := endpointFingerprint(resolved, key)
	if event == "success" {
		v.Failure = 0
		v.FailureEndpoint = ""
		v.Success = now
		v.SuccessEndpoint = finger
		v.Port = port
	} else {
		v.Failure = now
		v.FailureEndpoint = finger
	}
	h.Records[k] = v
	if len(h.Records) > 256 {
		keys := make([]string, 0, len(h.Records))
		for k := range h.Records {
			keys = append(keys, k)
		}
		sort.Slice(keys, func(i, j int) bool {
			a, b := h.Records[keys[i]], h.Records[keys[j]]
			return max(a.Success, a.Failure) < max(b.Success, b.Failure)
		})
		for len(h.Records) > 256 {
			delete(h.Records, keys[0])
			keys = keys[1:]
		}
	}
	return writeHistory(path, h)
}
func supportedProtocols(eps []Endpoint) []string {
	out := []string{}
	for _, p := range protocolNames {
		for _, e := range eps {
			key, err := base64.StdEncoding.DecodeString(e.PublicKey)
			_, _, _, ok := resolveProtocol(e.EntryIP, e.EntryPerProtocol, p)
			if e.Online && ok && err == nil && len(key) == 32 {
				out = append(out, p)
				break
			}
		}
	}
	return out
}
func applyObservations(nodes []Node, network, path string, now int64) {
	h := readHistory(path, bootID())
	for i := range nodes {
		n := &nodes[i]
		n.ProtocolState = "eligible"
		n.VerifiedAt = 0
		n.RetryAfter = 0
		v, ok := h.Records[historyKey(network, n.Protocol, n.Name)]
		if !ok {
			continue
		}
		successMatches, failureMatches := false, false
		for _, e := range n.Endpoints {
			ip, ports, _, ok := resolveProtocol(e.EntryIP, e.EntryPerProtocol, n.Protocol)
			if !ok {
				continue
			}
			finger := endpointFingerprint(ip, e.PublicKey)
			portOK := len(ports) == 0
			for _, p := range ports {
				if p == v.Port {
					portOK = true
				}
			}
			if finger == v.SuccessEndpoint && portOK {
				successMatches = true
			}
			if finger == v.FailureEndpoint {
				failureMatches = true
			}
		}
		if failureMatches && v.Failure >= v.Success && v.Failure <= now && now-v.Failure < failureCooldown {
			n.ProtocolState = "unreachable"
			n.RetryAfter = v.Failure + failureCooldown
		} else if successMatches && v.Success >= v.Failure && v.Success <= now && now-v.Success < verificationLifetime {
			n.ProtocolState = "verified"
			n.VerifiedAt = v.Success
		}
	}
}
func deprioritizeFailures(names []string, nodes []Node) []string {
	bad := map[string]bool{}
	for _, n := range nodes {
		bad[n.Name] = n.ProtocolState == "unreachable"
	}
	out := make([]string, 0, len(names))
	for _, n := range names {
		if !bad[n] {
			out = append(out, n)
		}
	}
	for _, n := range names {
		if bad[n] {
			out = append(out, n)
		}
	}
	return out
}
