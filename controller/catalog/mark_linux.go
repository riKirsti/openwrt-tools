//go:build linux

package main

import (
	"golang.org/x/sys/unix"
	"syscall"
)

func markSocket(c syscall.RawConn, mark int) error {
	var inner error
	err := c.Control(func(fd uintptr) { inner = unix.SetsockoptInt(int(fd), unix.SOL_SOCKET, unix.SO_MARK, mark) })
	if err != nil {
		return err
	}
	return inner
}
