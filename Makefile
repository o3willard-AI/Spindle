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
