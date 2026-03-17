# Deployment (Fedora)

## Host Layout

Deploy under:

- `/docker/ssh-hunt`

Persistent volumes:

- Postgres: `/docker/ssh-hunt/volumes/postgres`
- Game data/config: `/docker/ssh-hunt/volumes/ssh-hunt`
- Backups: `/docker/ssh-hunt/volumes/backups`

## Initial Setup

```bash
cd /docker/ssh-hunt
./scripts/install.sh
cp .env.example .env
make up
```

## Port Exposure

Default public port is `24444` mapped to container `22222`.

```bash
sudo firewall-cmd --permanent --add-port=24444/tcp
sudo firewall-cmd --reload
```

Repo helper target (opens `24444/tcp` in all firewalld zones):

```bash
make firewall-open-24444
make firewall-status
```

Edge routing requirements (outside host firewall):

- Router/NAT forward: `WAN TCP 24444 -> <server-lan-ip>:24444`
- If using public DNS A/AAAA records directly, keep record in DNS-only mode (no HTTP proxy layer for raw SSH).

## Runtime Secret Configuration

Set super-admin mapping in:

- `/docker/ssh-hunt/volumes/ssh-hunt/secrets/admin.yaml`
- `/docker/ssh-hunt/volumes/ssh-hunt/secrets/hidden_ops.yaml` (private hidden mission + optional Telegram relay)
- `/docker/ssh-hunt/volumes/ssh-hunt/secrets/ssh_host_ed25519` (persistent SSH host key)

Keep this file private and chmod 600.
Ensure `/docker/ssh-hunt/volumes/ssh-hunt/secrets` is writable by container user `10001:10001` if you want persistent SSH host keys.

## Migrations and Seeding

```bash
make db-migrate
make db-seed
```

## Backup / Restore

```bash
make backup
make restore
# or explicitly
./scripts/restore.sh ./volumes/backups/ssh-hunt-YYYYMMDD-HHMMSS.dump
```

## Upgrade Procedure

1. Pull latest repository changes.
2. Rebuild and restart:

```bash
make up
```

3. Apply migrations:

```bash
make db-migrate
```

4. Validate health:

```bash
make ps
make logs
make doctor
```

## Hardening Recommendations

- Enable CrowdSec or Fail2ban on host.
- Keep OS and Docker runtime patched.
- Restrict management SSH by IP where possible.
- Monitor failed SSH connection rates.
