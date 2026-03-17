SHELL := /bin/bash
COMPOSE := docker compose
RUNNER_COMPOSE := docker compose --env-file .env.runner -f docker-compose.runner.yml

.PHONY: ensure-env up down logs ps restart db-migrate db-seed test backup restore doctor firewall-open-24444 firewall-status runner-env runner-cpu-budget runner-workdirs runner-up runner-down runner-logs runner-ps

ensure-env:
	@if [ ! -f .env ]; then \
		cp .env.example .env; \
		echo "Created .env from .env.example"; \
	fi

up: ensure-env
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
	$(COMPOSE) run --rm --entrypoint /usr/local/bin/admin ssh-hunt migrate

db-seed:
	$(COMPOSE) run --rm --entrypoint /usr/local/bin/admin ssh-hunt seed

test:
	cd ssh-hunt && cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace --all-features

backup:
	./scripts/backup.sh

restore:
	./scripts/restore.sh

runner-env:
	@if [ ! -f .env.runner ]; then \
		cp .env.runner.example .env.runner; \
		echo "Created .env.runner from .env.runner.example"; \
	fi
	./scripts/refresh-runner-cpu-budget.sh .env.runner

runner-cpu-budget:
	./scripts/refresh-runner-cpu-budget.sh .env.runner

runner-workdirs:
	./scripts/prepare-runner-workdirs.sh

runner-up: runner-env
	$(MAKE) --no-print-directory runner-workdirs
	$(RUNNER_COMPOSE) up -d --build

runner-down:
	$(RUNNER_COMPOSE) down

runner-logs:
	$(RUNNER_COMPOSE) logs -f --tail=200

runner-ps:
	$(RUNNER_COMPOSE) ps

firewall-open-24444:
	@if command -v firewall-cmd >/dev/null 2>&1; then \
		zones="$$(timeout 10 firewall-cmd --get-zones 2>/dev/null || echo public)"; \
		for z in $$zones; do \
			timeout 10 sudo firewall-cmd --zone $$z --add-port=24444/tcp || true; \
			timeout 10 sudo firewall-cmd --permanent --zone $$z --add-port=24444/tcp || true; \
		done; \
		timeout 10 sudo firewall-cmd --reload || true; \
	else \
		echo "firewall-cmd not found; skipping firewalld updates"; \
	fi

firewall-status:
	@if command -v firewall-cmd >/dev/null 2>&1; then \
		fwc="firewall-cmd"; \
		if command -v sudo >/dev/null 2>&1 && sudo -n true >/dev/null 2>&1; then fwc="sudo -n firewall-cmd"; fi; \
		echo "== Active zones =="; \
		timeout 10 $$fwc --get-active-zones || true; \
		echo ""; \
		echo "== Port 24444/tcp in active zones =="; \
		zone_list="$$(timeout 10 $$fwc --get-active-zones 2>/dev/null | awk 'NF==1 {print $$1}')"; \
		zones_dump="$$(timeout 15 $$fwc --list-all-zones 2>/dev/null || true)"; \
		for z in $$zone_list; do \
			ports="$$(printf '%s\n' "$$zones_dump" | awk -v zone="$$z" '\
				/^[^ ]/ {current=$$1; inzone=(current==zone)} \
				inzone && /^  ports:/ {sub(/^  ports:[[:space:]]*/, "", $$0); print; exit}\
			')"; \
			if [ -z "$$ports" ]; then ports="<none>"; fi; \
			if printf ' %s ' "$$ports" | grep -q ' 24444/tcp '; then ok=yes; else ok=no; fi; \
			echo "$$z: 24444/tcp=$$ok ports=[$$ports]"; \
		done; \
	else \
		echo "firewall-cmd not found"; \
	fi

doctor:
	@echo "== Compose status =="
	@if docker info >/dev/null 2>&1; then \
		$(COMPOSE) ps; \
	else \
		echo "docker daemon unavailable or inaccessible; skipping compose status"; \
	fi
	@echo ""
	@echo "== Listener check (:24444) =="
	@ss -ltnp | grep ':24444' || true
	@echo ""
	@echo "== Public firewall ports (if firewalld available) =="
	@if command -v firewall-cmd >/dev/null 2>&1; then \
		$(MAKE) --no-print-directory firewall-status; \
	else \
		echo "firewall-cmd not found"; \
	fi
