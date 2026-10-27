include $(TOPDIR)/rules.mk
PKG_NAME:=proton-stealth-integration
PKG_VERSION:=1.0.0
PKG_RELEASE:=2
PKG_LICENSE:=GPL-3.0-or-later
include $(INCLUDE_DIR)/package.mk
define Package/proton-stealth-integration
  SECTION:=net
  CATEGORY:=Network
  TITLE:=Guarded Stealth adapter for the compatible exit-node controllers
  DEPENDS:=+protun-openwrt +curl +jsonfilter +ip-full
endef
define Build/Compile
endef
define Package/proton-stealth-integration/install
	$(INSTALL_DIR) $(1)/usr/lib/exitnode $(1)/usr/libexec $(1)/etc/init.d
	$(INSTALL_DATA) ./files/backend.sh $(1)/usr/lib/exitnode/
	$(INSTALL_BIN) ./files/proton-tunnel ./files/proton-stealth ./files/proton-transport-metadata $(1)/usr/libexec/
	$(INSTALL_BIN) ./files/proton-stealth.init $(1)/etc/init.d/proton-stealth
endef
$(eval $(call BuildPackage,proton-stealth-integration))
