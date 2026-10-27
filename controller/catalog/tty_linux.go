//go:build linux

package main

import "os"

func openTTY() (*os.File, error) { return os.Open("/dev/tty") }
