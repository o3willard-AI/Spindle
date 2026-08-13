.PHONY: test-up test-down test-reset test-logs test-clean clippy-ci test-all

COMPOSE := docker compose

## Test infrastructure targets
test-up: ## Start all test services and wait for healthy
	$(COMPOSE) up -d --wait
	@echo "==> Initializing MinIO bucket..."
	@sleep 2 && docker exec spindle-minio mc alias set local http://localhost:9000 minioadmin minioadmin >/dev/null 2>&1 \
		&& docker exec spindle-minio mc mb local/spindle-archive --ignore-existing \
		&& echo "==> Bucket 'spindle-archive' ready." || echo "==> WARNING: MinIO init skipped (container may still be starting)"

test-down: ## Stop and remove all test services + volumes
	$(COMPOSE) down -v --remove-orphans

test-reset: test-down test-up ## Full reset: destroy and rebuild test infra

test-logs: ## Show logs from all test services
	$(COMPOSE) logs -f

test-clean: ## Remove all containers, volumes, and networks
	$(COMPOSE) down -v --remove-orphans --rmi local

## Development helpers
test-ps: ## List running test containers
	$(COMPOSE) ps

test-exec-db: ## Execute shell in postgres container
	$(COMPOSE) exec postgres sh

test-exec-minio: ## Execute shell in minio container
	$(COMPOSE) exec minio sh

test-exec-keycloak: ## Execute shell in keycloak container
	$(COMPOSE) exec keycloak /bin/bash

## Database migration targets

# Spindle uses a custom migration approach (sqitch-style directory layout):
# - migrations/NNN_name/up.sql   — forward migration
# - migrations/NNN_name/down.sql — rollback migration
# Migrations are numbered (001–028), applied in order on `make migrate-up`
# and reversed in reverse order on `make migrate-down`.

# Database connection (override via environment)
DATABASE_URL ?= postgresql://spindle:spindle@localhost:5432/spindle

# Migration ordering (must match migrations/ directory listing)
MIGRATIONS := $(shell ls -1d migrations/*/ | sort)
MIGRATION_DIRS := $(shell ls -1d migrations/*/ | sort | tr '\n' ' ')

# Apply all up migrations in order
migrate-up: ## Apply all pending migrations (up)
	@cd $(shell dirname $(lastword $(MAKEFILE_LIST))) && \
	for dir in $(MIGRATION_DIRS); do \
		if [ -f "$$${dir}up.sql" ]; then \
			echo "==> Applying: $${dir}up.sql"; \
			psql "$$DATABASE_URL" -f "$$${dir}up.sql" || exit 1; \
		fi; \
	done
	@echo "==> All migrations applied."

# Apply all down migrations in reverse order
migrate-down: ## Roll back all migrations (down, in reverse order)
	@cd $(shell dirname $(lastword $(MAKEFILE_LIST))) && \
	for dir in $$(echo "$(MIGRATION_DIRS)" | tr ' ' '\n' | tac | tr '\n' ' '); do \
		if [ -f "$$${dir}down.sql" ]; then \
			echo "==> Rolling back: $${dir}down.sql"; \
			psql "$$DATABASE_URL" -f "$$${dir}down.sql" || exit 1; \
		else \
			echo "==> WARNING: No down.sql in $${dir} — skip"; \
		fi; \
	done
	@echo "==> All migrations rolled back."

# Apply a single migration up or down by number (requires NUM=NN)
migrate-up-n: NUM ?=
migrate-up-n: ## Apply single migration (usage: make migrate-up-n NUM=020)
ifeq ($(NUM),)
	@echo "ERROR: NUM is required. Usage: make migrate-up-n NUM=020"
	@exit 1
endif
	@dir=$$(ls -1d migrations/$(NUM)_*/ 2>/dev/null | head -1); \
	if [ -z "$$dir" ]; then echo "Migration $(NUM) not found"; exit 1; fi; \
	if [ -f "$${dir}up.sql" ]; then \
		echo "==> Applying: $${dir}up.sql"; \
		psql "$$DATABASE_URL" -f "$${dir}up.sql"; \
	fi

migrate-down-n: NUM ?=
migrate-down-n: ## Roll back single migration (usage: make migrate-down-n NUM=020)
ifeq ($(NUM),)
	@echo "ERROR: NUM is required. Usage: make migrate-down-n NUM=020"
	@exit 1
endif
	@dir=$$(ls -1d migrations/$(NUM)_*/ 2>/dev/null | head -1); \
	if [ -z "$$dir" ]; then echo "Migration $(NUM) not found"; exit 1; fi; \
	if [ -f "$${dir}down.sql" ]; then \
		echo "==> Rolling back: $${dir}down.sql"; \
		psql "$$DATABASE_URL" -f "$${dir}down.sql"; \
	else \
		echo "==> No down.sql for $(NUM) — backup-restore required (see docs/operator/rollback.md)"; \
	fi

## SBOM (Software Bill of Materials) targets

# The sbom target generates a CycloneDX SBOM (bom.json) at the repository root.
# This lists all workspace crates, their versions, and all third-party
# dependencies — useful for vulnerability scanning, license compliance,
# and supply-chain audits.
#
# Prerequisite: cargo-cyclonedx must be installed:
#   cargo install cargo-cyclonedx
#
# Output: bom.json — a CycloneDX JSON document (v1.5) at the repository root,
#   conforming to https://cyclonedx.org/schema/bom-1.5.schema.json
#
# Usage:
#   make sbom          # Generate bom.json at repo root
#   make sbom-clean    # Remove generated SBOM files

# cargo-cyclonedx v0.5.9 generates one file per workspace member crate as
# {crate_dir}/bom.json.json. We run on the workspace manifest and collect
# the main server crate's SBOM (which includes all transitive dependencies
# from the 23 workspace members) into a root-level bom.json.
sbom: ## Generate CycloneDX SBOM (bom.json) at repo root
	@echo "==> Generating SBOM with cargo-cyclonedx..."
	@cargo cyclonedx --manifest-path Cargo.toml --format json --spec-version 1.5 --override-filename bom -q
	@echo "==> Collecting root-level bom.json..."
	@mv spindle-server/bom.json bom.json
	@find . -mindepth 2 -name "bom.json" -not -path "./target/*" -delete; true
	@SIZE=$$(wc -c < bom.json); echo "==> SBOM generated: bom.json ($$SIZE bytes)"

sbom-clean: ## Remove generated SBOM files
	@find . -name "bom.json" -not -path "./target/*" -not -path "./.git/*" -delete
	@rm -f bom.json
	@echo "==> SBOM files cleaned"

sbom-check: ## Generate SBOM to stdout (CI verification, no file written)
	cargo cyclonedx --manifest-path Cargo.toml --format json --spec-version 1.5 -q

## Code quality targets

# Clippy with deny-warnings: any new warning becomes a hard error.
# This enforces the S-15 policy: clippy deny blocks new warnings.
#
# NOTE: spindle-bench pulls in libduckdb-sys which requires ~13GB disk for a
# full debug build. In CI/resource-constrained environments, skip with:
#   cargo clippy -p spindle-rawarchive -p spindle-server -p spindle-config -- -D warnings
clippy-ci: ## Run clippy with -D warnings (deny all warnings)
	cargo clippy --workspace --all-targets -- -D warnings

# Run all characterization + unit tests
test-all: ## Run all tests including characterization tests
	cargo test --workspace --all-targets
	bash tests/shell/test_clippy_deny_warnings.sh

## Release targets

# Binaries produced by the workspace:
#   spindle-server  — HTTP API + ingest server
#   spindle-worker  — pipeline processor daemon
#   spindle          — CLI (cargo bin name in spindle-cli crate)
#   spindle-migrate  — database migration runner
#
# IMPORTANT (glibc compatibility):
#   Ubuntu binaries MUST be built on Ubuntu 24.04 (glibc 2.39).

BINS := spindle-server spindle-worker spindle spindle-migrate
DIST_ROOT := dist
DIST_VERSION := $(shell git describe --tags --exact-match 2>/dev/null || echo "dev")
DIST_UBUNTU_VERSION := 24.04

# Build and strip all release binaries, place into target/release,
# then copy to dist/ubuntu/<version>/ with SHA256SUMS.
# Run on Ubuntu 24.04 for glibc 2.39 (see RELEASE.md).
release: ## Build all 4 binaries stripped + checksums for Ubuntu
	@echo "==> RELEASE: building on $$(lsb_release -ds 2>/dev/null || echo 'unknown')"
	@echo "==> RELEASE: Rust $$(rustc --version)"
	@echo "==> RELEASE: glibc $$(ldd --version | head -1)"
	@echo "==> RELEASE: cargo build --release (this may take several minutes)..."
	@cargo build --release --bin spindle-server --bin spindle-worker --bin spindle --bin spindle-migrate
	@echo "==> RELEASE: stripping binaries..."
	@for bin in $(BINS); do \
		strip --strip-all target/release/$$bin; \
		echo "  stripped: target/release/$$bin"; \
	done
	@echo "==> RELEASE: assembling dist tree (dist/ubuntu/$(DIST_VERSION)/)..."
	@DIST_DIR=$(DIST_ROOT)/ubuntu/$(DIST_VERSION); \
	mkdir -p $$DIST_DIR; \
	for bin in $(BINS); do \
		cp target/release/$$bin $$DIST_DIR/; \
		echo "  copied: $$bin"; \
	done
	@cd $(DIST_ROOT)/ubuntu/$(DIST_VERSION) && sha256sum $(BINS) > SHA256SUMS && \
		echo "==> RELEASE: SHA256SUMS:"; \
		cat SHA256SUMS | sed 's/^/  /'
	@echo "==> RELEASE: done. Artifacts in $(DIST_ROOT)/ubuntu/$(DIST_VERSION)/"
	@echo "==> NOTE: Run 'make dist-asm' to populate dist/ubuntu/$(DIST_UBUNTU_VERSION)/."

# Convenience: assemble dist/ubuntu/24.04 from build output.
# Run `make release` first, then `make dist-asm`.
dist-asm: ## Assemble dist/ubuntu/24.04/ from release output
	@echo "==> dist-asm: assembling from $(DIST_ROOT)/ubuntu/$(DIST_VERSION)/..."
	@if [ ! -d "$(DIST_ROOT)/ubuntu/$(DIST_VERSION)" ]; then \
		echo "ERROR: Run 'make release' first."; exit 1; \
	fi
	@mkdir -p $(DIST_ROOT)/ubuntu/$(DIST_UBUNTU_VERSION); \
	touch $(DIST_ROOT)/ubuntu/$(DIST_UBUNTU_VERSION)/.gitkeep; \
	for bin in $(BINS); do \
		cp $(DIST_ROOT)/ubuntu/$(DIST_VERSION)/$$bin $(DIST_ROOT)/ubuntu/$(DIST_UBUNTU_VERSION)/; \
	done; \
	cp $(DIST_ROOT)/ubuntu/$(DIST_VERSION)/SHA256SUMS $(DIST_ROOT)/ubuntu/$(DIST_UBUNTU_VERSION)/; \
	echo "  populated: $(DIST_ROOT)/ubuntu/$(DIST_UBUNTU_VERSION)/"

# Clean release artifacts
release-clean: ## Remove release binaries and dist tree
	@rm -f $(DIST_ROOT)/ubuntu/*/spindle-server $(DIST_ROOT)/ubuntu/*/spindle-worker $(DIST_ROOT)/ubuntu/*/spindle $(DIST_ROOT)/ubuntu/*/spindle-migrate
	@rm -f $(DIST_ROOT)/ubuntu/*/SHA256SUMS
	@find $(DIST_ROOT) -type d -empty -delete 2>/dev/null || true
	@echo "==> Release artifacts cleaned."

