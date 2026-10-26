TOOLS_PACKAGE_DIR:=$(dir $(realpath $(lastword $(MAKEFILE_LIST))))
include $(TOPDIR)/rules.mk

PKG_NAME:=tailscale-rs
PKG_VERSION:=0.6.1
PKG_RELEASE:=1

PKG_SOURCE:=$(PKG_NAME)-$(PKG_VERSION).tar.gz
PKG_SOURCE_URL:=https://codeload.github.com/tailscale/tailscale-rs/tar.gz/v$(PKG_VERSION)?
PKG_HASH:=360e280494c5a513540a4c73990e002d17e6787097e5ca254b8616ba257fefc8

PKG_LICENSE:=BSD-3-Clause
PKG_LICENSE_FILES:=LICENSE

TOOLS_TITLE:=Tailscale Rust C interface library
TOOLS_URL:=https://github.com/tailscale/tailscale-rs
TOOLS_DESCRIPTION:=Tailscale Rust shared library exposing the upstream C interface.
TOOLS_LIBRARY:=libtailscalers.so
TOOLS_CRATE:=ts_ffi

include $(TOOLS_PACKAGE_DIR)../rust-library.mk
