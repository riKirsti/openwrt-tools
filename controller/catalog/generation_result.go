package main

import (
	"encoding/json"
	"fmt"
	"io"
	"time"
)

// Generation metadata is independent of diagnostic text and never contains keys.
func generationResult(w io.Writer, machine bool, mode, serial string, expires int64, server, endpoint string, protocol ...string) error {
	selectedProtocol := ""
	if len(protocol) > 0 {
		selectedProtocol = protocol[0]
	}
	if machine {
		return json.NewEncoder(w).Encode(struct {
			Version  int    `json:"version"`
			Serial   string `json:"serial"`
			Expires  int64  `json:"expires"`
			Mode     string `json:"mode"`
			Server   string `json:"server"`
			Endpoint string `json:"endpoint"`
			Protocol string `json:"protocol,omitempty"`
		}{1, serial, expires, mode, server, endpoint, selectedProtocol})
	}
	_, err := fmt.Fprintf(w, "Certificate: %s, serial %s, expires %s\n", mode, serial, time.Unix(expires, 0).UTC().Format("2006-01-02 15:04 UTC"))
	return err
}
