//go:build !linux

package main

import (
	"fmt"
	"syscall"
)

func markSocket(c syscall.RawConn, mark int) error { return fmt.Errorf("SO_MARK requires Linux") }
