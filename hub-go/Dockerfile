# One image: the Go binary embeds and serves the built React app. Build from hub-go/:
#   docker build -t klarkhub hub-go
FROM node:24-alpine AS ui
WORKDIR /src/ui
COPY ui/package.json ui/package-lock.json ./
RUN npm ci
COPY ui/ ./
# vite builds into ../internal/ui/dist
RUN mkdir -p /src/internal/ui && npm run build

FROM golang:1.26-alpine AS go
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY cmd ./cmd
COPY internal ./internal
COPY --from=ui /src/internal/ui/dist ./internal/ui/dist
ARG VERSION=dev
RUN CGO_ENABLED=0 go build -trimpath -ldflags "-s -w -X github.com/muthuishere/klark/hub-go/internal/app.Version=${VERSION}" -o /out/klarkhub ./cmd/klarkhub
RUN mkdir /data

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=go /out/klarkhub /klarkhub
COPY --from=go --chown=65532:65532 /data /data
# Default sign-in: the built-in admin only (HUB_AUTH_MODE=mockldap opts in to a demo directory). HUB_LDAP_* and HUB_OIDC_PROVIDERS
# configure a real directory and more OAuth/OIDC providers; see docker-compose.yml and API.md.
# HUB_BACKUP=folder: SQLite snapshots and analytics files go to /data/backup (same volume: survives restarts, is not a
# backup of the volume; point HUB_BACKUP at s3 for that).
ENV HUB_ADDR=:7470 HUB_DATA_DIR=/data HUB_BACKUP=folder HUB_BACKUP_PATH=/data/backup
USER 65532:65532
EXPOSE 7470
VOLUME /data
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 CMD ["/klarkhub", "healthcheck"]
ENTRYPOINT ["/klarkhub"]
