.PHONY: test-up test-down test-reset

test-up:
	docker compose up -d --wait

test-down:
	docker compose down -v

test-reset: test-down test-up
