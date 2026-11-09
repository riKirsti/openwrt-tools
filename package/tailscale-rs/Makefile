TOOLS_PACKAGE_DIR:=$(dir $(realpath $(lastword $(MAKEFILE_LIST))))
SCAN_DEPS = ../rust-library.mk
include $(TOPDIR)/rules.mk

PKG_NAME:=tailscale-rs
PKG_VERSION:=0.7.1
PKG_RELEASE:=1

PKG_SOURCE:=$(PKG_NAME)-$(PKG_VERSION).tar.gz
PKG_SOURCE_URL:=https://codeload.github.com/tailscale/tailscale-rs/tar.gz/v$(PKG_VERSION)?
PKG_HASH:=e5ed53badbb9240161a5b4314f095776a2e7176ae84380c1849d2e86f28aa8a7

PKG_LICENSE:=BSD-3-Clause
PKG_LICENSE_FILES:=LICENSE

TOOLS_TITLE:=Tailscale Rust C interface library
TOOLS_URL:=https://github.com/tailscale/tailscale-rs
TOOLS_DESCRIPTION:=Tailscale Rust shared library exposing the upstream C interface.
TOOLS_LIBRARY:=libtailscalers.so
TOOLS_CRATE:=ts_ffi

include $(TOOLS_PACKAGE_DIR)../rust-library.mk

$(eval $(call BuildPackage,$(PKG_NAME)))
