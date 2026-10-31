package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/parquet-go/parquet-go"
)

type GenericRecord struct {
	ID        string `parquet:"id,snappy"`
	User      string `parquet:"user,snappy"`
	Score     string `parquet:"score,snappy"`
	Country   string `parquet:"country,snappy"`
	Timestamp string `parquet:"timestamp,snappy"`
	RawJSON   string `parquet:"raw_json,snappy"`
}

type IngestionServer struct {
	mu          sync.Mutex
	buffer      []GenericRecord
	destination string
	batchSize   int
}

func NewIngestionServer(dest string, batchSize int) *IngestionServer {
	return &IngestionServer{
		buffer:      make([]GenericRecord, 0, batchSize),
		destination: dest,
		batchSize:   batchSize,
	}
}

func (s *IngestionServer) handleHealth(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write([]byte("ok"))
}

func (s *IngestionServer) handleIngest(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	body, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, fmt.Sprintf("Failed to read body: %v", err), http.StatusBadRequest)
		return
	}
	defer r.Body.Close()

	if len(body) == 0 {
		http.Error(w, "Empty body", http.StatusBadRequest)
		return
	}

	var rawRecords []json.RawMessage
	if body[0] == '[' {
		if err := json.Unmarshal(body, &rawRecords); err != nil {
			http.Error(w, fmt.Sprintf("Invalid JSON array: %v", err), http.StatusBadRequest)
			return
		}
	} else if body[0] == '{' {
		rawRecords = append(rawRecords, body)
	} else {
		http.Error(w, "Expected JSON object or array", http.StatusBadRequest)
		return
	}

	var records []GenericRecord
	for _, raw := range rawRecords {
		var m map[string]interface{}
		if err := json.Unmarshal(raw, &m); err != nil {
			continue
		}

		rec := GenericRecord{
			RawJSON: string(raw),
		}
		if v, ok := m["id"]; ok {
			rec.ID = fmt.Sprintf("%v", v)
		}
		if v, ok := m["user"]; ok {
			rec.User = fmt.Sprintf("%v", v)
		}
		if v, ok := m["score"]; ok {
			rec.Score = fmt.Sprintf("%v", v)
		}
		if v, ok := m["country"]; ok {
			rec.Country = fmt.Sprintf("%v", v)
		}
		if v, ok := m["timestamp"]; ok {
			rec.Timestamp = fmt.Sprintf("%v", v)
		}
		records = append(records, rec)
	}

	s.mu.Lock()
	s.buffer = append(s.buffer, records...)
	shouldFlush := len(s.buffer) >= s.batchSize
	var toFlush []GenericRecord
	if shouldFlush {
		toFlush = s.buffer
		s.buffer = make([]GenericRecord, 0, s.batchSize)
	}
	s.mu.Unlock()

	if shouldFlush {
		go s.flushParquet(toFlush)
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write([]byte(fmt.Sprintf(`{"ingested":%d,"dlq":0}`, len(records))))
}

func (s *IngestionServer) flushParquet(records []GenericRecord) {
	if len(records) == 0 {
		return
	}

	buf := new(bytes.Buffer)
	writer := parquet.NewGenericWriter[GenericRecord](buf)
	if _, err := writer.Write(records); err != nil {
		log.Printf("[!] Parquet write error: %v", err)
		return
	}
	if err := writer.Close(); err != nil {
		log.Printf("[!] Parquet close error: %v", err)
		return
	}

	now := time.Now().UTC()
	hivePath := filepath.Join(
		s.destination,
		fmt.Sprintf("year=%04d/month=%02d/day=%02d", now.Year(), now.Month(), now.Day()),
	)
	_ = os.MkdirAll(hivePath, 0755)

	filePath := filepath.Join(hivePath, fmt.Sprintf("ingestr_%s.parquet", uuid.New().String()))
	if err := os.WriteFile(filePath, buf.Bytes(), 0644); err != nil {
		log.Printf("[!] Failed to save Parquet file: %v", err)
		return
	}
	log.Printf("[+] Wrote %d records to %s", len(records), filePath)
}

func main() {
	port := os.Getenv("PORT")
	if port == "" {
		port = "8080"
	}
	dest := os.Getenv("DESTINATION")
	if dest == "" {
		dest = "/tmp/ingestr_output"
	}
	batchSize := 500
	if bs := os.Getenv("BATCH_SIZE_RECORDS"); bs != "" {
		if val, err := strconv.Atoi(bs); err == nil && val > 0 {
			batchSize = val
		}
	}

	srv := NewIngestionServer(dest, batchSize)

	mux := http.NewServeMux()
	mux.HandleFunc("/health", srv.handleHealth)
	mux.HandleFunc("/ingest", srv.handleIngest)

	addr := fmt.Sprintf("0.0.0.0:%s", port)
	log.Printf("[*] Starting ingestr-go service on %s (dest=%s, batch=%d)", addr, dest, batchSize)
	if err := http.ListenAndServe(addr, mux); err != nil {
		log.Fatalf("Server failed: %v", err)
	}
}
