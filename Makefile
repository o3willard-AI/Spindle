.PHONY: test-up test-down test-reset test-logs test-clean

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

