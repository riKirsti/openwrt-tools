TOOLS_PACKAGE_DIR:=$(dir $(realpath $(lastword $(MAKEFILE_LIST))))
SCAN_DEPS = ../rust-library.mk
include $(TOPDIR)/rules.mk

PKG_NAME:=proton-account
PKG_VERSION:=0.1.0
PKG_RELEASE:=1
PKG_LICENSE:=GPL-3.0-or-later
PKG_BUILD_DIR:=$(BUILD_DIR)/$(PKG_NAME)-$(PKG_VERSION)
PKG_FILE_DEPENDS:=$(TOOLS_PACKAGE_DIR)Cargo.toml $(TOOLS_PACKAGE_DIR)Cargo.lock $(TOOLS_PACKAGE_DIR)src $(TOOLS_PACKAGE_DIR).cargo $(TOOLS_PACKAGE_DIR)files
TOOLS_TITLE:=Proton VPN account and WireGuard configuration client
TOOLS_URL:=https://github.com/riKirsti/openwrt-tools
TOOLS_DESCRIPTION:=Rust Proton account management and WireGuard configuration client.
TOOLS_DEPENDS:=+jsonfilter
include $(TOOLS_PACKAGE_DIR)../rust-library.mk

define Package/proton-account
  SECTION:=net
  CATEGORY:=Network
  TITLE:=$(TOOLS_TITLE)
  URL:=$(TOOLS_URL)
  DEPENDS:=$(RUST_ARCH_DEPENDS) +libgcc $(TOOLS_DEPENDS)
endef

define Build/Prepare
	$(INSTALL_DIR) $(PKG_BUILD_DIR)/src $(PKG_BUILD_DIR)/.cargo
	$(CP) ./Cargo.toml ./Cargo.lock $(PKG_BUILD_DIR)/
	$(CP) ./src/*.rs $(PKG_BUILD_DIR)/src/
	$(CP) ./.cargo/config.toml $(PKG_BUILD_DIR)/.cargo/
endef

define Build/Compile
	+cd $(PKG_BUILD_DIR) && $(TOOLS_CARGO_ENV) $(TOOLS_CARGO) build --locked --bin proton-account --profile $(CARGO_PKG_PROFILE) --target-dir $(TOOLS_CARGO_TARGET_DIR) $(if $(filter --jobserver%,$(PKG_JOBS)),,-j1)
	+cd $(PKG_BUILD_DIR) && $(TOOLS_CARGO_ENV) $(TOOLS_CARGO) test --locked --no-run --bin proton-account --profile $(CARGO_PKG_PROFILE) --target-dir $(TOOLS_CARGO_TARGET_DIR) --message-format=json $(if $(filter --jobserver%,$(PKG_JOBS)),,-j1) > $(PKG_BUILD_DIR)/proton-account-tests.jsonl
endef

define Package/$(PKG_NAME)/install
	$(INSTALL_DIR) $(1)/usr/libexec $(1)/usr/lib/exitnode
	$(INSTALL_BIN) $(TOOLS_CARGO_TARGET_DIR)/$(RUSTC_TARGET_ARCH)/$(CARGO_OUTPUT_DIR)/proton-account $(1)/usr/libexec/
	$(INSTALL_BIN) ./files/controller/proton-tunnel $(1)/usr/libexec/proton-kernel-tunnel
	$(INSTALL_DATA) ./files/controller/backend.sh $(1)/usr/lib/exitnode/kernel-backend.sh
endef

$(eval $(call BuildPackage,$(PKG_NAME)))
