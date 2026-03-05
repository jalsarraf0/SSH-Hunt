SHELL := /bin/bash
COMPOSE := docker compose

.PHONY: up down logs ps restart db-migrate db-seed test backup restore

up:
	$(COMPOSE) up -d --build

down:
	$(COMPOSE) down

logs:
	$(COMPOSE) logs -f --tail=200

ps:
	$(COMPOSE) ps

restart:
	$(COMPOSE) restart

db-migrate:
	$(COMPOSE) run --rm ssh-hunt /usr/local/bin/admin migrate

db-seed:
	$(COMPOSE) run --rm ssh-hunt /usr/local/bin/admin seed

test:
	cd ssh-hunt && cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace --all-features

backup:
	./scripts/backup.sh

restore:
	./scripts/restore.sh
