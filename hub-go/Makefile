.PHONY: dev test build docker ui

# API with the embedded UI as last built (run `make ui` first), on :7470, data in ./data
dev:
	HUB_MOCK_ADFS=1 HUB_DATA_DIR=./data go run ./cmd/klarkhub

# UI with fixtures (no hub needed): see ui/README
ui:
	cd ui && npm ci && npm run build

test:
	gofmt -l . | (! grep .)
	go vet ./...
	go test -race ./...
	cd ui && npm test

build: ui
	CGO_ENABLED=0 go build -trimpath -ldflags "-s -w" -o bin/klarkhub ./cmd/klarkhub

docker:
	docker build -t klarkhub:latest .
