// Lean router client using Proton official libraries and the Go HTTP transport.
// No code is imported from the community confgen project.
package main

import (
	"bytes"
	"cmp"
	"context"
	"crypto/subtle"
	"crypto/tls"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"syscall"
	"time"

	srp "github.com/ProtonMail/go-srp"
	"github.com/ProtonVPN/go-vpn-lib/ed25519"
)

type apiStatus struct{ Code int }

func (s apiStatus) responseCode() int { return s.Code }

type Session struct {
	apiStatus
	UID          string
	AccessToken  string
	RefreshToken string
	ExpiresIn    int
	Scopes       []string
	ServerProof  string
}
type Saved struct {
	Session   *Session  `json:"session"`
	Username  string    `json:"username"`
	SavedAt   time.Time `json:"saved_at"`
	ExpiresAt time.Time `json:"expires_at"`
}
type Physical struct {
	EntryPerProtocol                             map[string]ProtocolEntry
	ID, EntryIP, ExitIP, Domain, X25519PublicKey string
	Status                                       int
}
type Logical struct {
	ID, Name, Domain, EntryCountry, ExitCountry, HostCountry, City string
	Tier, Features, Status, Load                                   int
	Score                                                          float64
	Servers                                                        []Physical
}
type Certificate struct {
	apiStatus
	SerialNumber, Mode, DeviceName  string
	ClientKey, ClientKeyFingerprint string
	ExpirationTime, RefreshTime     int64
	Features                        json.RawMessage
}
type options struct {
	metadataJSON                                                                                                       bool
	api, username, server, output, serial, duration, countries, totp, hv                                               string
	list, json, free, ipv6, noSave, nonInteractive, passwordStdin, forceRefresh, logout, forget                        bool
	password, dns, allowed, device, renew, sessionDuration                                                             string
	listConfigs, withSessions, clearSession, noSession, p2p, secure, accelerator, forwarding, moderate, debug, cleanup bool
	netshield                                                                                                          int
	transport                                                                                                          string
	protocolDetails                                                                                                    bool
	explicit                                                                                                           map[string]bool
}
type Client struct {
	http           *http.Client
	base           string
	session        *Session
	persist        func(*Session) error
	netzone        string
	hvToken        string
	readTOTP       func() (string, error)
	publishCountry func(string)
}
type apiError struct {
	HTTP, Code int
	Methods    []string
	Challenge  string
}

func (e *apiError) Error() string {
	if e.Code == 9001 {
		return "PROTON_HUMAN_VERIFICATION_REQUIRED: CAPTCHA required; use secure interactive authorization"
	}
	if e.HTTP == 401 {
		return "PROTON_REAUTH_REQUIRED: authorization rejected"
	}
	return fmt.Sprintf("Proton API failed: HTTP %d code %d", e.HTTP, e.Code)
}

func newHTTP(timeout time.Duration) (*http.Client, error) {
	dial := net.Dialer{Timeout: timeout}
	if raw := os.Getenv("PROTON_WAN_MARK"); raw != "" {
		n, e := strconv.ParseUint(raw, 0, 32)
		if e != nil || n == 0 {
			return nil, errors.New("invalid control-plane mark")
		}
		dial.Control = func(_, _ string, c syscall.RawConn) error { return markSocket(c, int(n)) }
	}
	if raw := os.Getenv("PROTON_WAN_DNS"); raw != "" {
		host := strings.Split(raw, "%")[0]
		if net.ParseIP(host) == nil {
			return nil, errors.New("invalid bootstrap DNS")
		}
		dns := net.JoinHostPort(raw, "53")
		dial.Resolver = &net.Resolver{PreferGo: true, Dial: func(ctx context.Context, network, _ string) (net.Conn, error) {
			d := net.Dialer{Timeout: timeout, Control: dial.Control}
			return d.DialContext(ctx, network, dns)
		}}
	}
	tr := &http.Transport{DialContext: dial.DialContext, TLSClientConfig: &tls.Config{MinVersion: tls.VersionTLS12}, MaxIdleConns: 2, MaxIdleConnsPerHost: 2, IdleConnTimeout: 10 * time.Second, ResponseHeaderTimeout: timeout}
	return &http.Client{Transport: tr, Timeout: timeout, CheckRedirect: func(*http.Request, []*http.Request) error { return errors.New("API redirects disabled") }}, nil
}

func (c *Client) request(ctx context.Context, method, path string, body any) (*http.Request, error) {
	var encoded []byte
	var err error
	if body != nil {
		encoded, err = json.Marshal(body)
		if err != nil {
			return nil, err
		}
	}
	req, err := http.NewRequestWithContext(ctx, method, c.base+path, bytes.NewReader(encoded))
	if err != nil {
		return nil, err
	}
	arch := runtime.GOARCH
	if arch == "amd64" {
		arch = "x86-64"
	}
	if arch == "arm64" {
		arch = "aarch64"
	}
	req.Header.Set("x-pm-appversion", "linux-vpn-gui@5.8.7+"+arch)
	req.Header.Set("User-Agent", "ProtonVPN/5.8.7 (Linux; ubuntu/24.04)")
	req.Header.Set("Accept", "application/json")
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	if path == "/auth" && method == "POST" && c.hvToken != "" {
		req.Header.Set("x-pm-human-verification-token", c.hvToken)
		req.Header.Set("x-pm-human-verification-token-type", "captcha")
	}
	if c.session != nil {
		req.Header.Set("Authorization", "Bearer "+c.session.AccessToken)
		req.Header.Set("x-pm-uid", c.session.UID)
	}
	if strings.HasPrefix(path, "/vpn/v1/logicals") {
		if c.netzone != "" {
			req.Header.Set("X-PM-netzone", c.netzone)
		}
		req.Header.Set("If-Modified-Since", "Thu, 01 Jan 1970 00:00:00 GMT")
	}
	return req, nil
}
func (c *Client) call(ctx context.Context, method, path string, body any, out any, refresh bool) error {
	req, err := c.request(ctx, method, path, body)
	if err != nil {
		return err
	}
	resp, err := c.http.Do(req)
	if err != nil {
		return errors.New("API network request failed")
	}
	raw, readErr := io.ReadAll(io.LimitReader(resp.Body, 32*1024*1024+1))
	resp.Body.Close()
	if readErr != nil {
		return errors.New("API response read failed")
	}
	if len(raw) > 32*1024*1024 {
		return errors.New("API response too large")
	}
	if resp.StatusCode == 401 && refresh && c.session != nil {
		if err = c.refresh(ctx); err != nil {
			return err
		}
		return c.call(ctx, method, path, body, out, false)
	}
	var status apiStatus
	target := out
	if target == nil {
		target = &status
	}
	if err = json.Unmarshal(raw, target); err != nil {
		return fmt.Errorf("non-JSON API response: HTTP %d", resp.StatusCode)
	}
	coded, ok := target.(interface{ responseCode() int })
	if !ok {
		return errors.New("internal response type lacks API status")
	}
	code := coded.responseCode()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 || (code != 1000 && code != 1001) {
		var details struct {
			Details struct {
				HumanVerificationMethods []string
				HumanVerificationToken   string
			}
		}
		if code == 9001 {
			_ = json.Unmarshal(raw, &details)
		}
		return &apiError{HTTP: resp.StatusCode, Code: code, Methods: details.Details.HumanVerificationMethods, Challenge: details.Details.HumanVerificationToken}
	}
	return nil
}
func (c *Client) refresh(ctx context.Context) error {
	if c.session == nil || c.session.RefreshToken == "" {
		return errors.New("PROTON_REAUTH_REQUIRED")
	}
	body := map[string]any{"ResponseType": "token", "GrantType": "refresh_token", "RefreshToken": c.session.RefreshToken, "RedirectURI": "http://protonmail.ch"}
	var s Session
	if err := c.call(ctx, "POST", "/auth/refresh", body, &s, false); err != nil {
		return err
	}
	if s.UID == "" {
		s.UID = c.session.UID
	}
	if s.AccessToken == "" || s.RefreshToken == "" {
		return errors.New("incomplete refresh response")
	}
	if c.persist != nil {
		if err := c.persist(&s); err != nil {
			return err
		}
	}
	c.session = &s
	return nil
}
func (c *Client) login(ctx context.Context, user string, password []byte, totp string) error {
	defer clear(password)
	var info struct {
		apiStatus
		Version                                    int
		Salt, Modulus, ServerEphemeral, SRPSession string
	}
	if err := c.call(ctx, "POST", "/auth/info", map[string]string{"Username": user}, &info, false); err != nil {
		return err
	}
	a, err := srp.NewAuth(info.Version, user, password, info.Salt, info.Modulus, info.ServerEphemeral)
	if err != nil {
		return errors.New("invalid SRP challenge")
	}
	proof, err := a.GenerateProofs(2048)
	if err != nil {
		return errors.New("SRP proof generation failed")
	}
	body := map[string]string{"Username": user, "SRPSession": info.SRPSession, "ClientEphemeral": base64.StdEncoding.EncodeToString(proof.ClientEphemeral), "ClientProof": base64.StdEncoding.EncodeToString(proof.ClientProof)}
	var session Session
	if err = c.call(ctx, "POST", "/auth", body, &session, false); err != nil {
		return err
	}
	actual, e := base64.StdEncoding.DecodeString(session.ServerProof)
	if e != nil || subtle.ConstantTimeCompare(actual, proof.ExpectedServerProof) != 1 {
		return errors.New("server proof verification failed")
	}
	if session.UID == "" || session.AccessToken == "" || session.RefreshToken == "" {
		return errors.New("incomplete login response")
	}
	c.session = &session
	if !slices.Contains(session.Scopes, "vpn") {
		if totp == "" && c.readTOTP != nil {
			totp, err = c.readTOTP()
			if err != nil {
				c.session = nil
				return err
			}
		}
		if !slices.Contains(session.Scopes, "twofactor") || !validTOTP(totp) {
			c.session = nil
			return errors.New("TOTP required")
		}
		var two struct {
			apiStatus
			Scopes []string
		}
		if err = c.call(ctx, "POST", "/auth/2fa", map[string]string{"TwoFactorCode": totp}, &two, false); err != nil {
			c.session = nil
			return err
		}
		if !slices.Contains(two.Scopes, "vpn") {
			c.session = nil
			return errors.New("VPN scope missing")
		}
		session.Scopes = two.Scopes
	}
	session.ServerProof = ""
	if c.persist != nil {
		return c.persist(&session)
	}
	return nil
}
func (c *Client) servers(ctx context.Context) ([]Logical, error) {
	var loc struct {
		apiStatus
		IP, Country string
	}
	if err := c.call(ctx, "GET", "/vpn/v1/location", nil, &loc, true); err != nil {
		return nil, err
	}
	if ip := net.ParseIP(loc.IP).To4(); ip != nil {
		ip[3] = 0
		c.netzone = ip.String()
	}
	if c.publishCountry != nil {
		c.publishCountry(loc.Country)
	}
	var result struct {
		apiStatus
		LogicalServers []Logical
	}
	if err := c.call(ctx, "GET", "/vpn/v1/logicals?SecureCoreFilter=all&WithState=true", nil, &result, true); err != nil {
		return nil, err
	}
	return result.LogicalServers, nil
}
func (c *Client) certificate(ctx context.Context, pem string) (Certificate, error) {
	return c.certificateFor(ctx, pem, options{duration: "7d", noSave: true, accelerator: true})
}
func (c *Client) certificateFor(ctx context.Context, pem string, o options) (Certificate, error) {
	minutes, err := durationMinutes(o.duration)
	if err != nil {
		return Certificate{}, err
	}
	body := map[string]any{"ClientPublicKey": pem, "Duration": fmt.Sprintf("%d min", minutes), "Features": certificateFeatures(o)}
	if !o.noSave {
		body["Mode"] = "persistent"
		body["DeviceName"] = o.device
	}
	var cert Certificate
	err = c.call(ctx, "POST", "/vpn/v1/certificate", body, &cert, true)
	if err == nil && (cert.SerialNumber == "" || cert.ExpirationTime <= time.Now().Unix()) {
		err = errors.New("invalid certificate metadata")
	}
	return cert, err
}
func (c *Client) revoke(ctx context.Context, serial string) error {
	if serial == "" {
		return errors.New("empty serial")
	}
	var r struct {
		apiStatus
		Count int
	}
	if err := c.call(ctx, "DELETE", "/vpn/v1/certificate", map[string]string{"SerialNumber": serial}, &r, true); err != nil {
		return err
	}
	if r.Count != 1 {
		return errors.New("certificate not revoked")
	}
	return nil
}
func safeWrite(path string, data []byte) error {
	if fi, e := os.Lstat(path); e == nil && (!fi.Mode().IsRegular() || fi.Mode()&os.ModeSymlink != 0) {
		return errors.New("refusing non-regular destination")
	}
	dir := filepath.Dir(path)
	f, e := os.CreateTemp(dir, ".proton-lean-*")
	if e != nil {
		return e
	}
	name := f.Name()
	defer os.Remove(name)
	if e = f.Chmod(0600); e == nil {
		_, e = f.Write(data)
	}
	if e == nil {
		e = f.Sync()
	}
	closeErr := f.Close()
	if e != nil {
		return e
	}
	if closeErr != nil {
		return closeErr
	}
	return os.Rename(name, path)
}
func sessionPath() string {
	return filepath.Join(os.Getenv("PROTON_SESSION_DIR"), ".protonvpn-session.json")
}
func saveSession(user string, s *Session) error {
	now := time.Now().UTC()
	data, e := json.Marshal(Saved{s, user, now, now.Add(time.Duration(s.ExpiresIn) * time.Second)})
	if e != nil {
		return e
	}
	return safeWrite(sessionPath(), data)
}
func loadSession(user string) (*Saved, error) {
	if fi, e := os.Lstat(sessionPath()); e != nil {
		return nil, e
	} else if !fi.Mode().IsRegular() {
		return nil, errors.New("authorization is not a regular file")
	}
	data, e := os.ReadFile(sessionPath())
	if e != nil {
		return nil, e
	}
	var s Saved
	if e = json.Unmarshal(data, &s); e != nil {
		return nil, e
	}
	if s.Username != user || s.Session == nil {
		return nil, errors.New("authorization identity mismatch")
	}
	if s.Session.UID == "" || s.Session.AccessToken == "" || s.Session.RefreshToken == "" {
		return nil, errors.New("incomplete authorization")
	}
	return &s, nil
}

type endpointRow struct {
	Hostname         string                   `json:"hostname"`
	EntryIP          string                   `json:"entry_ip"`
	ExitIP           string                   `json:"exit_ip"`
	PublicKey        string                   `json:"public_key"`
	Online           bool                     `json:"online"`
	EntryPerProtocol map[string]ProtocolEntry `json:"entry_per_protocol,omitempty"`
}
type serverRow struct {
	Name         string        `json:"name"`
	Hostname     string        `json:"hostname"`
	Country      string        `json:"country"`
	EntryCountry string        `json:"entry_country"`
	HostCountry  string        `json:"host_country,omitempty"`
	City         string        `json:"city"`
	Tier         string        `json:"tier"`
	Load         int           `json:"load"`
	Score        float64       `json:"score"`
	Features     []string      `json:"features"`
	Endpoints    []endpointRow `json:"endpoints"`
}

func outputServers(all []Logical, o options) ([]serverRow, error) {
	selected := make([]Logical, 0, len(all))
	countries := strings.Split(o.countries, ",")
	for _, s := range all {
		if s.Status != 1 || len(s.Servers) == 0 || o.free != (s.Tier == 0) {
			continue
		}
		if o.countries != "" && !slices.Contains(countries, s.ExitCountry) {
			continue
		}
		if o.secure && s.Features&1 == 0 {
			continue
		}
		if o.p2p && !o.free && !o.secure && s.Features&4 == 0 {
			continue
		}
		selected = append(selected, s)
	}
	slices.SortFunc(selected, func(a, b Logical) int {
		if c := cmp.Compare(a.ExitCountry, b.ExitCountry); c != 0 {
			return c
		}
		return cmp.Compare(a.Score, b.Score)
	})
	out := make([]serverRow, 0, len(selected))
	for _, s := range selected {
		endpoints := make([]endpointRow, 0, len(s.Servers))
		for _, p := range s.Servers {
			endpoints = append(endpoints, endpointRow{p.Domain, p.EntryIP, p.ExitIP, p.X25519PublicKey, p.Status == 1, p.EntryPerProtocol})
		}
		features := []string{}
		for _, f := range []struct {
			bit  int
			name string
		}{{1, "SecureCore"}, {2, "Tor"}, {4, "P2P"}, {8, "Streaming"}, {16, "IPv6"}} {
			if s.Features&f.bit != 0 {
				features = append(features, f.name)
			}
		}
		tier := "Unknown"
		switch s.Tier {
		case 0:
			tier = "Free"
		case 2:
			tier = "Plus"
		case 3:
			tier = "ProtonMail"
		}
		row := serverRow{s.Name, s.Domain, s.ExitCountry, s.EntryCountry, "", s.City, tier, s.Load, s.Score, features, endpoints}
		if s.HostCountry != "" && s.HostCountry != s.ExitCountry {
			row.HostCountry = s.HostCountry
		}
		out = append(out, row)
	}
	if len(out) == 0 {
		return nil, errors.New("no online matching servers")
	}
	return out, nil
}
func generate(c *Client, ctx context.Context, o options) error {
	all, e := c.servers(ctx)
	if e != nil {
		return e
	}
	if o.transport != "" {
		all = c.withProtocolEntries(ctx, all)
	}
	selected, e := selectLogical(all, o)
	if e != nil {
		return e
	}
	var endpoint *Physical
	for i := range selected.Servers {
		p := &selected.Servers[i]
		resolved, _, _, ok := resolveProtocol(p.EntryIP, p.EntryPerProtocol, o.transport)
		key, err := base64.StdEncoding.DecodeString(p.X25519PublicKey)
		if p.Status == 1 && ok && err == nil && len(key) == 32 {
			copy := *p
			copy.EntryIP = resolved
			endpoint = &copy
			break
		}
	}
	if endpoint == nil || net.ParseIP(endpoint.EntryIP) == nil {
		return errors.New("no usable endpoint")
	}
	pk, e := base64.StdEncoding.DecodeString(endpoint.X25519PublicKey)
	if e != nil || len(pk) != 32 {
		return errors.New("invalid peer key")
	}
	pair, e := ed25519.NewKeyPair()
	if e != nil {
		return e
	}
	pem, e := pair.PublicKeyPKIXPem()
	if e != nil {
		return e
	}
	cert, e := c.certificateFor(ctx, pem, o)
	if e != nil {
		return e
	}
	address, dns, allowed := "10.2.0.2/32", "10.2.0.1", "0.0.0.0/0"
	if o.ipv6 {
		address += ", 2a07:b944::2:2/128"
		dns += ", 2a07:b944::2:1"
		allowed += ", ::/0"
	}
	if o.dns != "" {
		dns = strings.Join(strings.Split(o.dns, ","), ", ")
	}
	if o.allowed != "" {
		allowed = strings.Join(strings.Split(o.allowed, ","), ", ")
	}
	_, ports, _, _ := resolveProtocol(endpoint.EntryIP, endpoint.EntryPerProtocol, o.transport)
	port := 51820
	if protocolName(o.transport) != "wireguard-udp" {
		port = 443
	}
	if len(ports) > 0 {
		port = ports[0]
	}
	content := fmt.Sprintf("# Proton VPN controller credential\n[Interface]\nPrivateKey = %s\nAddress = %s\nDNS = %s\n\n[Peer]\nPublicKey = %s\nAllowedIPs = %s\nEndpoint = %s\n", pair.ToX25519Base64(), address, dns, endpoint.X25519PublicKey, allowed, net.JoinHostPort(endpoint.EntryIP, strconv.Itoa(port)))
	content = "# TransportProtocol = " + protocolName(o.transport) + "\n" + protocolPortsComment(ports) + content
	if e = safeWrite(o.output, []byte(content)); e != nil {
		if cleanup := c.revoke(ctx, cert.SerialNumber); cleanup != nil {
			return fmt.Errorf("configuration write failed; pending cleanup serial %s", cert.SerialNumber)
		}
		return e
	}
	mode := cert.Mode
	if mode == "" {
		mode = "session"
	}
	if e = generationResult(os.Stdout, o.metadataJSON, mode, cert.SerialNumber, cert.ExpirationTime, selected.Name, net.JoinHostPort(endpoint.EntryIP, strconv.Itoa(port)), protocolName(o.transport)); e != nil {
		if cleanup := c.revoke(ctx, cert.SerialNumber); cleanup != nil {
			return fmt.Errorf("result write failed; pending cleanup serial %s", cert.SerialNumber)
		}
		_ = os.Remove(o.output)
		return e
	}
	return nil
}
func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "Error:", err)
		os.Exit(1)
	}
}
