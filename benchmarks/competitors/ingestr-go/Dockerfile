# ==============================================================================
# ingestr (Go) Ingestion Service Dockerfile
# Multi-stage static build -> distroless/static-debian12
# ==============================================================================
FROM golang:1.23-bookworm AS builder

WORKDIR /app

COPY go.mod ./
RUN go mod tidy && go mod download

COPY main.go ./
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -ldflags="-s -w" -o /ingestr-bin main.go

FROM gcr.io/distroless/static-debian12:nonroot

WORKDIR /app
COPY --from=builder /ingestr-bin /usr/local/bin/ingestr

USER nonroot:nonroot
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/ingestr"]
