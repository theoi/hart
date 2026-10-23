################################################################################
#
# hart-demos
#
################################################################################

HART_DEMOS_VERSION = 1.0
HART_DEMOS_SITE = $(BR2_EXTERNAL_HART_PATH)/package/hart-demos/src
HART_DEMOS_SITE_METHOD = local
HART_DEMOS_LICENSE = MIT or Apache-2.0
HART_DEMOS_LICENSE_FILES = LICENSE-MIT LICENSE-APACHE

# -ffp-contract=off keeps a*b+c as two rounded operations, so the output can
# also be reproduced on hosts without fused multiply-add.
define HART_DEMOS_BUILD_CMDS
	$(TARGET_CC) $(TARGET_CFLAGS) -O2 -ffp-contract=off -Wall -Wextra \
		$(TARGET_LDFLAGS) -o $(@D)/mandelbrot $(@D)/mandelbrot.c
endef

define HART_DEMOS_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/mandelbrot $(TARGET_DIR)/usr/bin/mandelbrot
endef

$(eval $(generic-package))
