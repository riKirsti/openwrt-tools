package main

import (
	"os"
	"testing"
)

func TestProductionSessionDirectoryDefault(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("USERPROFILE", home)
	t.Setenv("PROTON_SESSION_DIR", "")
	if err := initSessionDirectory(false); err == nil {
		t.Fatal("loopback fixture accepted without explicit isolated directory")
	}
	if err := initSessionDirectory(true); err != nil {
		t.Fatal(err)
	}
	if os.Getenv("PROTON_SESSION_DIR") != home {
		t.Fatal("production did not use the user's home directory")
	}
	explicit := t.TempDir()
	t.Setenv("PROTON_SESSION_DIR", explicit)
	if err := initSessionDirectory(true); err != nil || os.Getenv("PROTON_SESSION_DIR") != explicit {
		t.Fatal("explicit session directory not preserved")
	}
}
