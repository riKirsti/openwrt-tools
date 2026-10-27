package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net/url"
	"os"
	"time"
)

func (c *Client) certificates(ctx context.Context, withSessions bool) ([]Certificate, error) {
	out := []Certificate{}
	begin := ""
	seen := map[string]bool{}
	for page := 0; page < 10000; page++ {
		filter := "Mode=persistent"
		if withSessions {
			filter = "WithSessions=1"
		}
		path := "/vpn/v1/certificate/all?" + filter + "&Limit=50"
		if begin != "" {
			path += "&BeginID=" + url.QueryEscape(begin)
		}
		var response struct {
			apiStatus
			Certificates []Certificate
		}
		if e := c.call(ctx, "GET", path, nil, &response, true); e != nil {
			return nil, e
		}
		out = append(out, response.Certificates...)
		if len(response.Certificates) < 50 {
			return out, nil
		}
		begin = response.Certificates[len(response.Certificates)-1].SerialNumber
		if begin == "" || seen[begin] {
			return nil, errors.New("invalid certificate pagination")
		}
		seen[begin] = true
	}
	return nil, errors.New("certificate page limit exceeded")
}
func featureJSON(raw json.RawMessage) map[string]any {
	var features map[string]any
	if json.Unmarshal(raw, &features) != nil || len(features) == 0 {
		return nil
	}
	out := map[string]any{}
	boolValue := func(v any) (bool, bool) {
		switch x := v.(type) {
		case bool:
			return x, true
		case float64:
			return x != 0, true
		}
		return false, false
	}
	if x, ok := features["NetShieldLevel"].(float64); ok {
		out["netshield"] = int(x)
	}
	for _, p := range []struct {
		api, json string
		invert    bool
	}{{"RandomNAT", "moderate_nat", true}, {"PortForwarding", "port_forwarding", false}, {"SplitTCP", "accelerator", false}} {
		if v, ok := boolValue(features[p.api]); ok {
			if p.invert {
				v = !v
			}
			out[p.json] = v
		}
	}
	return out
}
func listCertificates(c *Client, ctx context.Context, o options) error {
	certs, e := c.certificates(ctx, o.withSessions)
	if e != nil {
		return e
	}
	rows := make([]map[string]any, 0, len(certs))
	for _, cert := range certs {
		row := map[string]any{"serial": cert.SerialNumber, "mode": cert.Mode, "device_name": cert.DeviceName, "expires": time.Unix(cert.ExpirationTime, 0).UTC(), "fingerprint": cert.ClientKeyFingerprint}
		if f := featureJSON(cert.Features); f != nil {
			row["features"] = f
		}
		rows = append(rows, row)
	}
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	return enc.Encode(rows)
}
func renewCertificate(c *Client, ctx context.Context, o options) error {
	certs, e := c.certificates(ctx, false)
	if e != nil {
		return e
	}
	var target *Certificate
	for i := range certs {
		if certs[i].SerialNumber == o.renew {
			target = &certs[i]
			break
		}
	}
	if target == nil || target.ClientKey == "" || target.DeviceName == "" {
		return errors.New("persistent credential not found or incomplete")
	}
	var features map[string]any
	_ = json.Unmarshal(target.Features, &features)
	if features == nil {
		features = certificateFeatures(o)
	}
	newFeatures := certificateFeatures(o)
	for flag, key := range map[string]string{"netshield": "NetShieldLevel", "moderate-nat": "RandomNAT", "port-forwarding": "PortForwarding", "accelerator": "SplitTCP"} {
		if o.explicit[flag] {
			features[key] = newFeatures[key]
		}
	}
	minutes, e := durationMinutes(o.duration)
	if e != nil {
		return e
	}
	body := map[string]any{"ClientPublicKey": target.ClientKey, "Duration": fmt.Sprintf("%d min", minutes), "Features": features, "Mode": "persistent", "DeviceName": target.DeviceName, "Renew": true}
	var result Certificate
	if e = c.call(ctx, "POST", "/vpn/v1/certificate", body, &result, true); e != nil {
		return e
	}
	if result.SerialNumber == "" {
		return errors.New("renewal returned no serial")
	}
	fmt.Printf("Certificate renewed: %s -> %s\n", target.SerialNumber, result.SerialNumber)
	return nil
}
