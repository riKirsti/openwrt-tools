TOOLS_PACKAGE_DIR:=$(dir $(realpath $(lastword $(MAKEFILE_LIST))))
SCAN_DEPS = ../rust-library.mk
include $(TOPDIR)/rules.mk

PKG_NAME:=tailscale-rs
PKG_VERSION:=0.7.1
PKG_RELEASE:=2

PKG_SOURCE_COMMIT:=a2a56c41b1db72d50d59d71d35c6669ff9639d14
PKG_SOURCE:=$(PKG_NAME)-$(PKG_SOURCE_COMMIT).tar.gz
PKG_SOURCE_URL:=https://codeload.github.com/tailscale/tailscale-rs/tar.gz
PKG_SOURCE_URL_FILE:=$(PKG_SOURCE_COMMIT)
PKG_HASH:=d0fb7402aae4a5afe4360d94133382156f7e74d61b1fd2c61970cd5bdcd952d0
PKG_BUILD_DIR:=$(BUILD_DIR)/$(PKG_NAME)-$(PKG_SOURCE_COMMIT)

PKG_LICENSE:=BSD-3-Clause
PKG_LICENSE_FILES:=LICENSE

TOOLS_TITLE:=Tailscale Rust C interface library
TOOLS_URL:=https://github.com/tailscale/tailscale-rs
TOOLS_DESCRIPTION:=Tailscale Rust shared library exposing the upstream C interface.
TOOLS_LIBRARY:=libtailscalers.so
TOOLS_CRATE:=ts_ffi

TAILSCALE_HEADER:=$(PKG_BUILD_DIR)/crates/bindings/ts_ffi/tailscale.h

define TOOLS_EXTRA_INSTALL
	$(INSTALL_DIR) $(1)/usr/include
	$(INSTALL_DATA) $(TAILSCALE_HEADER) $(1)/usr/include/
endef

include $(TOOLS_PACKAGE_DIR)../rust-library.mk

define Build/InstallDev
	$(INSTALL_DIR) $(1)/usr/include $(1)/usr/lib
	$(INSTALL_DATA) $(TAILSCALE_HEADER) $(1)/usr/include/
	$(CP) $(TOOLS_CARGO_TARGET_DIR)/$(RUSTC_TARGET_ARCH)/$(CARGO_OUTPUT_DIR)/$(TOOLS_LIBRARY) $(1)/usr/lib/
endef

$(eval $(call BuildPackage,$(PKG_NAME)))
