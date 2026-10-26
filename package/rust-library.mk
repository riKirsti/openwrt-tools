# Shared build rules for this feed's Rust shared libraries.
PKG_FILE_DEPENDS+=$(realpath $(TOOLS_PACKAGE_DIR)../rust-library.mk)
PKG_CONFIG_DEPENDS+=TOOLS_BUILD_MODE TOOLS_RUST_ROOT RUST_VERSION CONFIG_DEBUG
PKG_BUILD_PARALLEL:=1
ifeq ($(strip $(TOOLS_RUST_ROOT)),)
  PKG_BUILD_DEPENDS:=rust/host
endif

include $(INCLUDE_DIR)/package.mk
include $(TOPDIR)/feeds/packages/lang/rust/rust-package.mk

TOOLS_MANIFEST?=Cargo.toml
TOOLS_CRATE?=$(PKG_NAME)
TOOLS_CARGO:=cargo
ifneq ($(strip $(TOOLS_RUST_ROOT)),)
  TOOLS_CARGO:=$(TOOLS_RUST_ROOT)/bin/cargo
  TOOLS_RUST_VARS:=RUSTC="$(TOOLS_RUST_ROOT)/bin/rustc"
  # Link against the SDK's musl and startup objects.
  CARGO_RUSTFLAGS+=-Clink-self-contained=no
endif

TOOLS_BUILD_MODE?=runtime
ifeq ($(TOOLS_BUILD_MODE),runtime)
  TOOLS_PROFILE_VARS:=CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_OPT_LEVEL=3
else ifeq ($(TOOLS_BUILD_MODE),fast)
  TOOLS_PROFILE_VARS:=CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 CARGO_PROFILE_RELEASE_OPT_LEVEL=1
else
  $(error TOOLS_BUILD_MODE must be runtime or fast)
endif

CARGO_OUTPUT_DIR:=$(if $(filter dev,$(CARGO_PKG_PROFILE)),debug,$(CARGO_PKG_PROFILE))
TOOLS_CARGO_TARGET_DIR?=$(PKG_BUILD_DIR)/target

define Package/$(PKG_NAME)
  SECTION:=net
  CATEGORY:=Network
  TITLE:=$(TOOLS_TITLE)
  URL:=$(TOOLS_URL)
  DEPENDS:=$(RUST_ARCH_DEPENDS) +libgcc
endef

define Package/$(PKG_NAME)/description
  $(TOOLS_DESCRIPTION)
endef

define Build/Prepare
	$(call Build/Prepare/Default)
	$(CP) ./files/Cargo.lock $(PKG_BUILD_DIR)/$(dir $(TOOLS_MANIFEST))Cargo.lock
endef

define Build/Compile
	+cd $(PKG_BUILD_DIR) && \
		$(CARGO_PKG_VARS) \
		$(TOOLS_PROFILE_VARS) \
		TARGET_AR="$(TARGET_AR)" \
		TARGET_CXX="$(TARGET_CXX_NOCACHE)" \
		TARGET_CXXFLAGS="$(TARGET_CXXFLAGS) $(RUSTC_CFLAGS)" \
		$(TOOLS_RUST_VARS) \
		$(TOOLS_CARGO) rustc -v --locked --lib --crate-type cdylib \
			--package $(TOOLS_CRATE) --manifest-path $(TOOLS_MANIFEST) \
			--profile $(CARGO_PKG_PROFILE) \
			$(if $(TOOLS_FEATURES),--features $(TOOLS_FEATURES)) \
			--target-dir $(TOOLS_CARGO_TARGET_DIR) \
			$(if $(filter --jobserver%,$(PKG_JOBS)),,-j1)
endef

define Package/$(PKG_NAME)/install
	$(INSTALL_DIR) $(1)/usr/lib
	$(INSTALL_BIN) $(TOOLS_CARGO_TARGET_DIR)/$(RUSTC_TARGET_ARCH)/$(CARGO_OUTPUT_DIR)/$(TOOLS_LIBRARY) $(1)/usr/lib/
endef

$(eval $(call BuildPackage,$(PKG_NAME)))
