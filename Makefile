# Variables
BINARY_NAME = mixed
TARGET_DIR = target/release
INSTALL_PATH = /usr/local/bin

.PHONY: all build install uninstall clean test

all: build

# 1. Compile the highly optimized production binary
build:
	@echo "Compiling optimized release binary..."
	cargo build --release

# 2. Deploy to the global system environment
install: build
	@echo "Checking if $(INSTALL_PATH) is in PATH..."
	@echo $$PATH | grep -q "$(INSTALL_PATH)" && echo "Path validation passed." || echo "WARNING: $(INSTALL_PATH) is not in your PATH variable!"
	@echo "Installing $(BINARY_NAME) to $(INSTALL_PATH)..."
	@mkdir -p $(INSTALL_PATH)
	@cp -f $(TARGET_DIR)/$(BINARY_NAME) $(INSTALL_PATH)/$(BINARY_NAME)
	@chmod +x $(INSTALL_PATH)/$(BINARY_NAME)
	@echo "Installation successful! You can now run the app from anywhere by typing '$(BINARY_NAME)'."

# 3. Complete system removal
uninstall:
	@echo "Removing $(BINARY_NAME) from $(INSTALL_PATH)..."
	@rm -f $(INSTALL_PATH)/$(BINARY_NAME)
	@echo "Uninstallation successful."

test:
	cargo test

clean:
	cargo clean
