TOOLS_PACKAGE_DIR:=$(dir $(realpath $(lastword $(MAKEFILE_LIST))))
SCAN_DEPS = ../rust-library.mk
include $(TOPDIR)/rules.mk

PKG_NAME:=protun
PKG_VERSION:=3.0.5
PKG_RELEASE:=1

PKG_SOURCE:=$(PKG_NAME)-$(PKG_VERSION).tar.gz
PKG_SOURCE_URL:=https://codeload.github.com/ProtonVPN/protun/tar.gz/v$(PKG_VERSION)?
PKG_HASH:=a95a3cce00cfb48f604c65fc21f985b751c4b04a19beb03917f61bb0379d70ee

PKG_LICENSE:=GPL-3.0-or-later
PKG_LICENSE_FILES:=LICENSE

TOOLS_TITLE:=Proton VPN core library
TOOLS_URL:=https://github.com/ProtonVPN/protun
TOOLS_DESCRIPTION:=Proton VPN core shared library for Linux.
TOOLS_LIBRARY:=libprotun.so
TOOLS_FEATURES:=linux

include $(TOOLS_PACKAGE_DIR)../rust-library.mk

$(eval $(call BuildPackage,$(PKG_NAME)))
