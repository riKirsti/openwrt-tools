package main

import (
	"bufio"
	"cmp"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/url"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"

	"golang.org/x/term"
)

func validTOTP(s string) bool {
	if len(s) != 6 {
		return false
	}
	for _, c := range s {
		if c < '0' || c > '9' {
			return false
		}
	}
	return true
}
func parseDuration(s string) (time.Duration, error) {
	if strings.HasSuffix(s, "d") {
		n, e := strconv.ParseInt(strings.TrimSuffix(s, "d"), 10, 32)
		if e != nil || n < 0 || n > 365 {
			return 0, errors.New("invalid day duration")
		}
		return time.Duration(n) * 24 * time.Hour, nil
	}
	return time.ParseDuration(s)
}
func durationMinutes(s string) (int64, error) {
	d, e := parseDuration(s)
	if e != nil || d < 10*time.Minute || d > 365*24*time.Hour || d%time.Minute != 0 {
		return 0, errors.New("invalid certificate duration")
	}
	return int64(d / time.Minute), nil
}
func certificateFeatures(o options) map[string]any {
	return map[string]any{"RandomNAT": !o.moderate, "SplitTCP": o.accelerator, "PortForwarding": o.forwarding, "NetShieldLevel": o.netshield}
}
func selectLogical(all []Logical, o options) (*Logical, error) {
	var best *Logical
	countries := strings.Split(o.countries, ",")
	for i := range all {
		s := &all[i]
		if s.Status != 1 {
			continue
		}
		// Named-server behavior matches the existing backend; outer controller validates catalog membership.
		if o.server != "" {
			if s.Name == o.server {
				return s, nil
			}
			continue
		}
		if len(s.Servers) == 0 || o.free != (s.Tier == 0) {
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
		if best == nil || cmp.Compare(s.Score, best.Score) < 0 || (s.Score == best.Score && s.Load < best.Load) {
			best = s
		}
	}
	if best == nil {
		return nil, errors.New("no online matching server")
	}
	return best, nil
}
func readPassword(r io.Reader) ([]byte, error) {
	p, e := io.ReadAll(io.LimitReader(r, 4097))
	if e != nil {
		return nil, e
	}
	p = stringsTrimPassword(p)
	if len(p) == 0 || len(p) > 4096 || strings.ContainsAny(string(p), "\r\n") {
		clear(p)
		return nil, errors.New("stdin must contain one password line")
	}
	return p, nil
}
func stringsTrimPassword(p []byte) []byte {
	if len(p) > 0 && p[len(p)-1] == '\n' {
		p = p[:len(p)-1]
		if len(p) > 0 && p[len(p)-1] == '\r' {
			p = p[:len(p)-1]
		}
	}
	return p
}
func promptLine(reader *bufio.Reader, prompt string) (string, error) {
	fmt.Fprint(os.Stderr, prompt)
	s, e := reader.ReadString('\n')
	if e != nil {
		return "", e
	}
	return strings.TrimSpace(s), nil
}
func publishCountry(country string) {
	if len(country) != 2 || country[0] < 'A' || country[0] > 'Z' || country[1] < 'A' || country[1] > 'Z' {
		return
	}
	conn, e := net.DialTimeout("unix", "/var/run/proton-country.sock", 200*time.Millisecond)
	if e != nil {
		return
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(200 * time.Millisecond))
	_, _ = fmt.Fprintln(conn, "set "+country)
	_, _ = bufio.NewReader(conn).ReadString('\n')
}
func (c *Client) verify(ctx context.Context, refresh bool) error {
	req, e := c.request(ctx, "GET", "/vpn/v1/logicals", nil)
	if e != nil {
		return e
	}
	resp, e := c.http.Do(req)
	if e != nil {
		return errors.New("API network request failed")
	}
	resp.Body.Close()
	// Like the deployed client, verification needs HTTP status, not a second catalog parse.
	if resp.StatusCode == 401 && refresh {
		if e = c.refresh(ctx); e != nil {
			return e
		}
		return c.verify(ctx, false)
	}
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return &apiError{HTTP: resp.StatusCode}
	}
	return nil
}
func parseOptions(args []string) (options, error) {
	var o options
	f := flag.NewFlagSet("proton-lean", flag.ContinueOnError)
	f.SetOutput(io.Discard)
	f.StringVar(&o.api, "api-url", "https://vpn-api.proton.me", "Proton API endpoint (numeric loopback is also allowed for local tests)")
	f.StringVar(&o.username, "username", "", "account identity")
	f.StringVar(&o.password, "password", "", "compatibility only; prefer terminal")
	f.StringVar(&o.server, "server", "", "exact server")
	f.StringVar(&o.transport, "transport", "wireguard-udp", "wireguard-udp, wireguard-tcp or stealth")
	f.BoolVar(&o.protocolDetails, "protocol-details", false, "retain protocol-specific server entry data")
	f.StringVar(&o.output, "output", "protonvpn.conf", "configuration output")
	f.StringVar(&o.serial, "revoke-serial", "", "certificate serial")
	f.StringVar(&o.duration, "duration", "365d", "certificate duration")
	f.StringVar(&o.countries, "countries", "", "country filter")
	f.StringVar(&o.totp, "fixture-totp", "", "synthetic test TOTP")
	f.StringVar(&o.hv, "hv-token", os.Getenv("PROTONVPN_HV_TOKEN"), "human verification replay")
	f.StringVar(&o.dns, "dns", "", "DNS list")
	f.StringVar(&o.allowed, "allowed-ips", "", "allowed CIDRs")
	f.StringVar(&o.device, "device-name", "", "persistent device name")
	f.StringVar(&o.sessionDuration, "session-duration", "0", "authorization cache duration")
	f.StringVar(&o.renew, "renew-serial", "", "persistent certificate renewal")
	f.BoolVar(&o.list, "list-servers", false, "catalog query")
	f.BoolVar(&o.json, "json", false, "JSON output")
	f.BoolVar(&o.metadataJSON, "metadata-json", false, "versioned generated-credential metadata without private keys")
	f.BoolVar(&o.free, "free-only", false, "free catalog filter")
	f.BoolVar(&o.ipv6, "ipv6", false, "dual stack")
	f.BoolVar(&o.noSave, "no-save", false, "temporary certificate")
	f.BoolVar(&o.nonInteractive, "non-interactive", false, "do not prompt")
	f.BoolVar(&o.passwordStdin, "password-stdin", false, "password from stdin")
	f.BoolVar(&o.forceRefresh, "force-refresh", false, "refresh authorization")
	f.BoolVar(&o.logout, "logout", false, "online logout")
	f.BoolVar(&o.forget, "forget", false, "local authorization removal")
	f.BoolVar(&o.clearSession, "clear-session", false, "clear authorization before login")
	f.BoolVar(&o.noSession, "no-session", false, "do not load or save authorization")
	f.BoolVar(&o.p2p, "p2p-only", true, "P2P filter")
	f.BoolVar(&o.secure, "secure-core", false, "Secure Core filter")
	f.BoolVar(&o.accelerator, "accelerator", true, "accelerator feature")
	f.BoolVar(&o.forwarding, "port-forwarding", false, "forwarding feature")
	f.BoolVar(&o.moderate, "moderate-nat", false, "moderate NAT feature")
	f.IntVar(&o.netshield, "netshield", 0, "NetShield level")
	f.BoolVar(&o.debug, "debug", false, "diagnostic compatibility")
	f.BoolVar(&o.listConfigs, "list-configs", false, "certificate list")
	f.BoolVar(&o.withSessions, "with-sessions", false, "include session certificates")
	if e := f.Parse(args); e != nil {
		if errors.Is(e, flag.ErrHelp) {
			f.SetOutput(os.Stderr)
			f.PrintDefaults()
			return o, flag.ErrHelp
		}
		return o, errors.New("invalid command arguments")
	}
	if protocolAPI(o.transport) == "" {
		return o, errors.New("invalid transport")
	}
	if f.NArg() != 0 {
		return o, errors.New("unexpected positional arguments")
	}
	o.username = strings.TrimSpace(o.username)
	for _, suffix := range []string{"@protonmail.com", "@proton.me", "@pm.me"} {
		o.username = strings.TrimSuffix(o.username, suffix)
	}
	normalizeList := func(raw string, upper bool) string {
		items := []string{}
		for _, item := range strings.Split(raw, ",") {
			item = strings.TrimSpace(item)
			if upper {
				item = strings.ToUpper(item)
			}
			if item != "" {
				items = append(items, item)
			}
		}
		return strings.Join(items, ",")
	}
	o.countries = normalizeList(o.countries, true)
	o.dns = normalizeList(o.dns, false)
	o.allowed = normalizeList(o.allowed, false)
	o.explicit = map[string]bool{}
	f.Visit(func(x *flag.Flag) { o.explicit[x.Name] = true })
	if o.noSave && !o.explicit["duration"] {
		o.duration = "7d"
	}
	if o.countries != "" {
		for _, c := range strings.Split(o.countries, ",") {
			if len(c) != 2 || c[0] < 'A' || c[0] > 'Z' || c[1] < 'A' || c[1] > 'Z' {
				return o, errors.New("invalid country")
			}
		}
	}
	if o.passwordStdin && o.password != "" {
		return o, errors.New("password input options conflict")
	}
	if o.passwordStdin && o.username == "" {
		return o, errors.New("password stdin requires explicit username")
	}
	if o.nonInteractive && (o.passwordStdin || o.password != "" || o.clearSession || o.noSession) {
		return o, errors.New("non-interactive authorization options conflict")
	}
	if o.forwarding && o.moderate {
		return o, errors.New("port forwarding conflicts with moderate NAT")
	}
	if o.netshield < 0 || o.netshield > 2 {
		return o, errors.New("invalid NetShield level")
	}
	if o.dns != "" {
		for _, ip := range strings.Split(o.dns, ",") {
			if net.ParseIP(strings.Split(ip, "%")[0]) == nil {
				return o, errors.New("invalid DNS address")
			}
		}
	}
	if o.allowed != "" {
		for _, prefix := range strings.Split(o.allowed, ",") {
			if _, _, e := net.ParseCIDR(prefix); e != nil {
				return o, errors.New("invalid allowed prefix")
			}
		}
	}
	d, e := durationMinutes(o.duration)
	if e != nil {
		return o, e
	}
	if o.noSave && d > 7*24*60 {
		return o, errors.New("temporary certificate duration exceeds seven days")
	}
	if o.sessionDuration != "0" {
		if d, e := parseDuration(o.sessionDuration); e != nil || d <= 0 {
			return o, errors.New("invalid session cache duration")
		}
	}
	return o, nil
}
func printChallenge(err error, base string) {
	var a *apiError
	if !errors.As(err, &a) || a.Code != 9001 {
		return
	}
	// Challenge metadata is shown only to the interactive terminal, never generic error logs.
	if !term.IsTerminal(int(os.Stderr.Fd())) {
		return
	}
	if a.Challenge != "" {
		fmt.Fprintln(os.Stderr, "CAPTCHA:", base+"/core/v4/captcha?Token="+url.QueryEscape(a.Challenge))
	}
	fmt.Fprintln(os.Stderr, "Solve in the browser; replay the combined challenge:response via PROTONVPN_HV_TOKEN.")
}
func removeAuthorization() error {
	e := os.Remove(sessionPath())
	if os.IsNotExist(e) {
		return nil
	}
	return e
}
func initSessionDirectory(official bool) error {
	if os.Getenv("PROTON_SESSION_DIR") != "" {
		return nil
	}
	if !official {
		return errors.New("explicit isolated session directory required for local tests")
	}
	home, err := os.UserHomeDir()
	if err != nil || home == "" {
		return errors.New("home directory unavailable; set PROTON_SESSION_DIR")
	}
	return os.Setenv("PROTON_SESSION_DIR", home)
}
func run() error {
	o, e := parseOptions(os.Args[1:])
	if errors.Is(e, flag.ErrHelp) {
		return nil
	}
	if e != nil {
		return e
	}
	u, e := url.Parse(o.api)
	officialAPI := o.api == "https://vpn-api.proton.me"
	if e != nil || u.User != nil || u.RawQuery != "" || u.Fragment != "" || u.Path != "" || (u.Scheme != "http" && u.Scheme != "https") || (!net.ParseIP(u.Hostname()).IsLoopback() && !officialAPI) {
		return errors.New("only the official Proton API or numeric loopback test endpoints are allowed")
	}
	if err := initSessionDirectory(officialAPI); err != nil {
		return err
	}
	name := strings.TrimSuffix(filepath.Base(os.Args[0]), ".exe")
	if name == "proton-session-revoke" {
		o.logout = true
	}
	if o.forget {
		return removeAuthorization()
	}
	if o.clearSession {
		if e = removeAuthorization(); e != nil {
			return e
		}
	}
	if o.logout && o.username == "" {
		raw, e := os.ReadFile(sessionPath())
		if e != nil {
			return e
		}
		var saved Saved
		if e = json.Unmarshal(raw, &saved); e != nil {
			return e
		}
		o.username = saved.Username
	}
	input := bufio.NewReader(os.Stdin)
	if o.username == "" {
		if o.nonInteractive {
			return errors.New("PROTON_REAUTH_REQUIRED: account identity missing")
		}
		o.username, e = promptLine(input, "Username: ")
		if e != nil || o.username == "" {
			return errors.New("account identity required")
		}
	}
	h, e := newHTTP(10 * time.Second)
	if e != nil {
		return e
	}
	defer h.CloseIdleConnections()
	c := &Client{http: h, base: o.api, hvToken: o.hv, publishCountry: publishCountry}
	if !o.noSession {
		c.persist = func(s *Session) error {
			now := time.Now().UTC()
			expires := now.Add(time.Duration(s.ExpiresIn) * time.Second)
			if o.sessionDuration != "0" {
				d, _ := parseDuration(o.sessionDuration)
				if local := now.Add(d); local.Before(expires) {
					expires = local
				}
			}
			data, e := json.Marshal(Saved{s, o.username, now, expires})
			if e != nil {
				return e
			}
			return safeWrite(sessionPath(), data)
		}
	}
	c.readTOTP = func() (string, error) {
		if o.nonInteractive {
			return "", errors.New("PROTON_REAUTH_REQUIRED: interactive TOTP required")
		}
		reader := input
		if o.passwordStdin {
			tty, e := openTTY()
			if e != nil {
				return "", errors.New("TOTP requires an interactive terminal")
			}
			defer tty.Close()
			reader = bufio.NewReader(tty)
		}
		code, e := promptLine(reader, "2FA Code: ")
		if e != nil || !validTOTP(code) {
			return "", errors.New("six digit TOTP required")
		}
		return code, nil
	}
	ctx := context.Background()
	var saved *Saved
	if !o.noSession {
		saved, e = loadSession(o.username)
	}
	if saved != nil && e == nil {
		c.session = saved.Session
		if o.logout {
			e = c.call(ctx, "DELETE", "/auth", nil, nil, true)
			if e != nil {
				return e
			}
			return removeAuthorization()
		}
		if e = c.call(ctx, "GET", "/tests/ping", nil, nil, false); e != nil {
			return e
		}
		if o.forceRefresh || time.Until(saved.ExpiresAt) < 7*24*time.Hour {
			e = c.refresh(ctx)
		} else {
			e = c.verify(ctx, true)
		}
		if e != nil {
			return e
		}
	} else {
		if o.logout {
			if os.IsNotExist(e) {
				return nil
			}
			return errors.New("saved authorization cannot be read")
		}
		if o.nonInteractive {
			return errors.New("PROTON_REAUTH_REQUIRED: saved authorization unavailable")
		}
		var password []byte
		switch {
		case o.passwordStdin:
			password, e = readPassword(os.Stdin)
		case o.password != "":
			password = []byte(o.password)
		default:
			fmt.Fprint(os.Stderr, "Password: ")
			password, e = term.ReadPassword(int(os.Stdin.Fd()))
			fmt.Fprintln(os.Stderr)
		}
		if e != nil {
			return e
		}
		if e = c.login(ctx, o.username, password, o.totp); e != nil {
			printChallenge(e, o.api)
			return e
		}
	}
	switch {
	case o.serial != "":
		return c.revoke(ctx, o.serial)
	case o.listConfigs:
		return listCertificates(c, ctx, o)
	case o.renew != "":
		return renewCertificate(c, ctx, o)
	case o.list:
		servers, e := c.servers(ctx)
		if e != nil {
			return e
		}
		if o.protocolDetails {
			servers = c.withProtocolEntries(ctx, servers)
		}
		rows, e := outputServers(servers, o)
		if e != nil {
			return e
		}
		if o.json {
			enc := json.NewEncoder(os.Stdout)
			enc.SetIndent("", "  ")
			return enc.Encode(rows)
		}
		for _, s := range rows {
			fmt.Printf("%s\t%s\t%s\t%d\t%.2f\n", s.Country, s.Name, s.City, s.Load, s.Score)
		}
		return nil
	default:
		if o.server == "" && o.countries == "" {
			return errors.New("server or countries required")
		}
		if !o.noSave && o.device == "" {
			o.device = "OpenWrt-WireGuard"
		}
		return generate(c, ctx, o)
	}
}
