// klarkhub is the klark hub: a Go API with the React UI embedded, in one binary.
//
//	klarkhub                 run the hub (configured by HUB_* environment variables)
//	klarkhub healthcheck     exit 0 when the local hub answers /healthz (for the container HEALTHCHECK)
//	klarkhub hash-password   print a bcrypt hash for HUB_USERS (reads the password from stdin)
//	klarkhub hash-token      print the sha256 of a machine token for HUB_MACHINES (reads stdin)
package main

import (
	"bufio"
	"context"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/app"
	"github.com/muthuishere/klark/hub-go/internal/auth"
	"github.com/muthuishere/klark/hub-go/internal/config"
	"github.com/muthuishere/klark/hub-go/internal/tokens"
)

func main() {
	if len(os.Args) > 1 {
		switch os.Args[1] {
		case "healthcheck":
			os.Exit(healthcheck())
		case "hash-password", "hash-token":
			line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
			line = strings.TrimRight(line, "\r\n")
			if os.Args[1] == "hash-token" {
				fmt.Println(tokens.Sha256Hex(line))
				return
			}
			h, err := auth.HashPassword(line)
			if err != nil {
				log.Fatal(err)
			}
			fmt.Println(h)
			return
		case "version":
			fmt.Println(app.Version)
			return
		}
	}
	cfg, err := config.FromEnv(os.Getenv)
	if err != nil {
		log.Fatal(err)
	}
	a, err := app.New(cfg, nil)
	if err != nil {
		log.Fatal(err)
	}
	defer a.Close()

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()
	go a.Registry.Run(ctx)

	srv := &http.Server{Addr: cfg.Addr, Handler: a.Handler(), ReadHeaderTimeout: 10 * time.Second, IdleTimeout: 120 * time.Second}
	done := make(chan struct{})
	go func() {
		<-ctx.Done()
		log.Printf("shutting down")
		sc, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		// Held streams (machine links, SSE) would keep Shutdown waiting: drop them at the deadline.
		if err := srv.Shutdown(sc); err != nil {
			_ = srv.Close()
		}
		close(done)
	}()
	log.Printf("klarkhub %s listening on %s (base path %q, data %s)", app.Version, cfg.Addr, cfg.BasePath, cfg.DataDir)
	if err := srv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		log.Fatal(err)
	}
	<-done
}

// healthcheck asks this machine's own hub for /healthz.
func healthcheck() int {
	addr := os.Getenv("HUB_ADDR")
	if addr == "" {
		addr = ":7470"
	}
	host, port, err := net.SplitHostPort(addr)
	if err != nil {
		return 1
	}
	if host == "" || host == "0.0.0.0" || host == "::" {
		host = "127.0.0.1"
	}
	c := http.Client{Timeout: 3 * time.Second}
	resp, err := c.Get("http://" + net.JoinHostPort(host, port) + "/healthz")
	if err != nil {
		return 1
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return 1
	}
	return 0
}
