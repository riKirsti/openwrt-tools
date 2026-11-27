TOOLS_PACKAGE_DIR:=$(dir $(realpath $(lastword $(MAKEFILE_LIST))))
SCAN_DEPS = ../rust-library.mk
include $(TOPDIR)/rules.mk

PKG_NAME:=protun-openwrt
PKG_VERSION:=3.0.5
PKG_RELEASE:=17

PKG_SOURCE:=protunnext-$(PKG_VERSION).tar.gz
PKG_SOURCE_COMMIT:=ba15cc9af2101742d2ead52c6b365db99c788f42
PKG_BUILD_DIR:=$(BUILD_DIR)/ProtunNext-$(PKG_SOURCE_COMMIT)
PKG_SOURCE_URL:=https://codeload.github.com/riKirsti/ProtunNext/tar.gz
PKG_SOURCE_URL_FILE:=$(PKG_SOURCE_COMMIT)
PKG_HASH:=b38c24e16657c96c96bc9d76790e2daeaf965cd5717f4ece4f25c532ceccb45b

PKG_LICENSE:=GPL-3.0-or-later
PKG_LICENSE_FILES:=LICENSE

TOOLS_TITLE:=Proton VPN protocol client
TOOLS_URL:=https://github.com/riKirsti/ProtunNext
TOOLS_DESCRIPTION:=OpenWrt Proton VPN client with UDP, TCP, Stealth and Local Agent support.
TOOLS_CRATE:=protun-next
TOOLS_DEPENDS:=+kmod-tun +jsonfilter

define TOOLS_EXTRA_PREPARE
	$(CP) ./files/openwrt-*.rs $(PKG_BUILD_DIR)/
endef

PROTUN_CLIENT_FLAGS = $(TOOLS_CARGO_FLAGS) --bin protun-openwrt \
	--no-default-features --features openwrt-client

define TOOLS_EXTRA_COMPILE
	+cd $(PKG_BUILD_DIR) && $(TOOLS_CARGO_ENV) \
		$(TOOLS_CARGO) build -v $(PROTUN_CLIENT_FLAGS)
	+cd $(PKG_BUILD_DIR) && $(TOOLS_CARGO_ENV) \
		$(TOOLS_CARGO) test $(PROTUN_CLIENT_FLAGS) --no-run --message-format=json \
			> $(PKG_BUILD_DIR)/openwrt-tests.jsonl
endef

define TOOLS_EXTRA_INSTALL
	$(INSTALL_DIR) $(1)/usr/sbin $(1)/usr/libexec $(1)/usr/lib/exitnode \
		$(1)/etc/init.d $(1)/etc/config $(1)/etc/protun
	$(INSTALL_BIN) $(TOOLS_CARGO_TARGET_DIR)/$(RUSTC_TARGET_ARCH)/$(CARGO_OUTPUT_DIR)/protun-openwrt $(1)/usr/sbin/
	$(INSTALL_BIN) ./files/protun.init $(1)/etc/init.d/protun
	$(INSTALL_CONF) ./files/protun.config $(1)/etc/config/protun
	$(INSTALL_DATA) ./files/*.json.example $(1)/etc/protun/
	$(INSTALL_DATA) ./files/controller/backend.sh $(1)/usr/lib/exitnode/
	$(INSTALL_BIN) ./files/controller/proton-tunnel ./files/controller/proton-stealth ./files/controller/proton-transport-metadata $(1)/usr/libexec/
	$(INSTALL_BIN) ./files/controller/proton-stealth.init $(1)/etc/init.d/proton-stealth
endef

include $(TOOLS_PACKAGE_DIR)../rust-library.mk

# This package installs an executable and optional controller helpers; override the helper's library targets.
define Build/Compile
	$(TOOLS_EXTRA_COMPILE)
endef

define Package/$(PKG_NAME)/install
	$(TOOLS_EXTRA_INSTALL)
endef

define Package/$(PKG_NAME)/conffiles
/etc/config/protun
/etc/protun/
endef

$(eval $(call BuildPackage,$(PKG_NAME)))
