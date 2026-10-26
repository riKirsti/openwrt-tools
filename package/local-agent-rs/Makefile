TOOLS_PACKAGE_DIR:=$(dir $(realpath $(lastword $(MAKEFILE_LIST))))
include $(TOPDIR)/rules.mk

PKG_NAME:=local-agent-rs
PKG_VERSION:=1.6.3
PKG_RELEASE:=1

PKG_SOURCE:=$(PKG_NAME)-$(PKG_VERSION).tar.gz
PKG_SOURCE_URL:=https://codeload.github.com/ProtonVPN/local-agent-rs/tar.gz/$(PKG_VERSION)?
PKG_HASH:=2558807482dcd56c1fb35535698292f1198706bf9f9286c9afd0ad4c8567081c

PKG_LICENSE:=unknown
# This upstream tag supplies no license file or manifest license declaration.

TOOLS_TITLE:=Proton local-agent Rust interface library
TOOLS_URL:=https://github.com/ProtonVPN/local-agent-rs
TOOLS_DESCRIPTION:=Proton local-agent Rust core with UniFFI bindings.
TOOLS_LIBRARY:=liblocal_agent_rs.so
TOOLS_MANIFEST:=local_agent_rs/Cargo.toml
TOOLS_FEATURES:=uniffi

include $(TOOLS_PACKAGE_DIR)../rust-library.mk
