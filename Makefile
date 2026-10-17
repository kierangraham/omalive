PREFIX ?= $(HOME)/.local
TARGET_DIR ?= target

.PHONY: run demo build check test shell-test lint probe promo film-looks install plugin uninstall clean
run:
	cargo run --locked --release -p omalive-app --target-dir "$(TARGET_DIR)"
demo:
	cargo run --locked -p omalive-app --target-dir "$(TARGET_DIR)" -- --demo
build:
	cargo build --locked --release -p omalive-app --target-dir "$(TARGET_DIR)"
check:
	cargo check --locked --workspace --all-targets --target-dir "$(TARGET_DIR)"
test:
	cargo test --locked --workspace --target-dir "$(TARGET_DIR)"
# The bar widget: its model in node, then the QML against a demo instance in
# a hidden Quickshell. Add --render to save PNGs to shell/tests/output.
shell-test:
	cargo build --locked -p omalive-app --target-dir "$(TARGET_DIR)"
	OMALIVE_BIN="$(TARGET_DIR)/debug/omalive" node shell/tests/model.test.cjs
	OMALIVE_BIN="$(abspath $(TARGET_DIR))/debug/omalive" shell/tests/qml-smoke
# The promo video: records the real app and bar widget in demo mode on a
# headless Hyprland output, then cuts artifacts/promo/omalive-promo.mp4.
promo:
	cargo build --locked --release -p omalive-app --target-dir "$(TARGET_DIR)"
	promo/record-app.sh
	PROMO_LAYOUT=zoom promo/record-bar.sh
	PROMO_LAYOUT=panel promo/record-bar.sh
	promo/compose.py
# Film-emulation looks from RawTherapee's collection (CC BY-SA 4.0).
film-looks: install
	scripts/get-film-looks
lint:
	cargo fmt --all -- --check
	cargo clippy --locked --workspace --all-targets --target-dir "$(TARGET_DIR)" -- -D warnings
# Live checks against the desktop session: voice chain, then camera with a
# PipeWire consumer. Uses your first microphone and camera.
probe:
	cargo build --locked --release -p omalive-media --examples --target-dir "$(TARGET_DIR)"
	"$(TARGET_DIR)/release/examples/probe" voice
	"$(TARGET_DIR)/release/examples/probe" camera 8 & sleep 2; "$(TARGET_DIR)/release/examples/consume" 4; wait
# The app: binary, desktop entry and icon. The bar widget is a separate
# step: `omarchy plugin add https://github.com/kierangraham/omalive --enable`.
install: build
	install -Dm755 "$(TARGET_DIR)/release/omalive" "$(PREFIX)/bin/omalive"
	install -Dm644 packaging/omalive.desktop "$(PREFIX)/share/applications/omalive.desktop"
	install -Dm644 assets/omalive.svg "$(PREFIX)/share/icons/hicolor/scalable/apps/omalive.svg"
	@command -v update-desktop-database >/dev/null && update-desktop-database "$(PREFIX)/share/applications" || true
# Development: copy this checkout's bar widget into the Omarchy shell instead
# of a git clone.
plugin:
	OMALIVE_BIN="$(PREFIX)/bin/omalive" shell/install
uninstall:
	rm -f "$(PREFIX)/bin/omalive" "$(PREFIX)/share/applications/omalive.desktop" "$(PREFIX)/share/icons/hicolor/scalable/apps/omalive.svg"
	rm -f "$${XDG_CONFIG_HOME:-$$HOME/.config}/autostart/omalive.desktop"
	@dir="$${XDG_CONFIG_HOME:-$$HOME/.config}/omarchy/plugins/kierangraham.omalive"; \
	  if [ -f "$$dir/.omalive-managed" ]; then omarchy plugin disable kierangraham.omalive || true; rm -rf -- "$$dir"; \
	  elif [ -d "$$dir" ]; then echo "Remove the bar widget with: omarchy plugin remove kierangraham.omalive"; fi
clean:
	cargo clean --target-dir "$(TARGET_DIR)"
