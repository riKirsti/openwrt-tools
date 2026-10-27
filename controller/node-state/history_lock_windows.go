package main

import (
	"os"
	"syscall"
)

func lockHistory(path string) (func(), error) {
	p, e := syscall.UTF16PtrFromString(path)
	if e != nil {
		return nil, e
	}
	h, e := syscall.CreateFile(p, syscall.GENERIC_READ|syscall.GENERIC_WRITE, 0, nil, syscall.OPEN_ALWAYS, syscall.FILE_ATTRIBUTE_NORMAL, 0)
	if e != nil {
		return nil, e
	}
	f := os.NewFile(uintptr(h), path)
	return func() { f.Close() }, nil
}
