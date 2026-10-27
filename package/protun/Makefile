TOOLS_PACKAGE_DIR:=$(dir $(realpath $(lastword $(MAKEFILE_LIST))))
SCAN_DEPS = ../rust-library.mk
include $(TOPDIR)/rules.mk

PKG_NAME:=protun
PKG_VERSION:=3.0.5
PKG_RELEASE:=2

PKG_SOURCE:=$(PKG_NAME)-$(PKG_VERSION).tar.gz
PKG_SOURCE_URL:=https://codeload.github.com/ProtonVPN/protun/tar.gz/v$(PKG_VERSION)?
PKG_HASH:=a95a3cce00cfb48f604c65fc21f985b751c4b04a19beb03917f61bb0379d70ee

PKG_LICENSE:=GPL-3.0-or-later
PKG_LICENSE_FILES:=LICENSE

TOOLS_TITLE:=Proton VPN core library and Stealth client
TOOLS_URL:=https://github.com/ProtonVPN/protun
TOOLS_DESCRIPTION:=Proton VPN core library and an experimental OpenWrt Stealth client.
TOOLS_LIBRARY:=libprotun.so
TOOLS_FEATURES:=linux
TOOLS_DEPENDS:=+kmod-tun +jsonfilter

define TOOLS_EXTRA_PREPARE
	$(CP) ./files/openwrt-client.rs $(PKG_BUILD_DIR)/openwrt-client.rs
endef

define TOOLS_EXTRA_COMPILE
	+cd $(PKG_BUILD_DIR) && $(TOOLS_CARGO_ENV) \
		$(TOOLS_CARGO) build -v --locked --bin protun-openwrt \
			--features openwrt-client \
			--profile $(CARGO_PKG_PROFILE) --target-dir $(TOOLS_CARGO_TARGET_DIR) \
			$(if $(filter --jobserver%,$(PKG_JOBS)),,-j1)
endef

define TOOLS_EXTRA_INSTALL
	$(INSTALL_DIR) $(1)/usr/sbin $(1)/etc/init.d $(1)/etc/config $(1)/etc/protun
	$(INSTALL_BIN) $(TOOLS_CARGO_TARGET_DIR)/$(RUSTC_TARGET_ARCH)/$(CARGO_OUTPUT_DIR)/protun-openwrt $(1)/usr/sbin/
	$(INSTALL_BIN) ./files/protun.init $(1)/etc/init.d/protun
	$(INSTALL_CONF) ./files/protun.config $(1)/etc/config/protun
	$(INSTALL_DATA) ./files/client.json.example $(1)/etc/protun/client.json.example
endef

include $(TOOLS_PACKAGE_DIR)../rust-library.mk

define Package/protun/conffiles
/etc/config/protun
/etc/protun/
endef

$(eval $(call BuildPackage,$(PKG_NAME)))
