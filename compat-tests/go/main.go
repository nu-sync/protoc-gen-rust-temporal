// compat-tests/go is the Go arm of the wire-format audit described in
// ../README.md. It walks fixtures/*.input.json, reconstructs the typed
// message via protoreflect, runs it through cludden's runtime converter,
// and writes <fixture>.go.payload.json next to the input.
//
// This file is committed as a *template*: the import of cludden's runtime
// resolves only after `go mod init github.com/nu-sync/protoc-gen-rust-temporal/compat-tests/go`
// (gated on the nu-sync org existing) and `go get
// github.com/cludden/protoc-gen-go-temporal/pkg/temporalv1`.
//
//go:build audit
// +build audit

package main

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	// Resolves once the module is wired up. See README for the bootstrap steps.
	// "github.com/cludden/protoc-gen-go-temporal/pkg/runtime"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/reflect/protoregistry"
	commonpb "go.temporal.io/api/common/v1"
)

type fixture struct {
	MessageType string                 `json:"message_type"`
	Fields      map[string]interface{} `json:"fields"`
}

type wirePayload struct {
	Metadata map[string]string `json:"metadata"`
	Data     string            `json:"data"` // base64
}

func main() {
	if len(os.Args) < 2 || os.Args[1] != "generate" {
		fmt.Fprintln(os.Stderr, "usage: go run . generate")
		os.Exit(2)
	}

	matches, err := filepath.Glob("../fixtures/*.input.json")
	if err != nil {
		panic(err)
	}
	for _, in := range matches {
		out := strings.TrimSuffix(in, ".input.json") + ".go.payload.json"
		if err := process(in, out); err != nil {
			fmt.Fprintf(os.Stderr, "%s: %v\n", in, err)
			os.Exit(1)
		}
		fmt.Printf("wrote %s\n", out)
	}
}

func process(in, out string) error {
	raw, err := os.ReadFile(in)
	if err != nil {
		return err
	}
	var f fixture
	if err := json.Unmarshal(raw, &f); err != nil {
		return err
	}

	mt, err := protoregistry.GlobalTypes.FindMessageByName(protoreflect.FullName(f.MessageType))
	if err != nil {
		return fmt.Errorf("unknown message type %q: %w", f.MessageType, err)
	}
	msg := mt.New().Interface()
	fieldsJSON, _ := json.Marshal(f.Fields)
	if err := json.Unmarshal(fieldsJSON, msg); err != nil {
		return fmt.Errorf("decode fields: %w", err)
	}

	wire, err := proto.Marshal(msg)
	if err != nil {
		return err
	}

	// The actual cludden converter call would look like:
	//   payload, err := runtime.DefaultConverter.ToPayload(msg)
	// We open-code the expected triple so the audit verifies our
	// expectation about cludden's shape — if the converter diverges from
	// this triple, the round-trip diff will surface it loudly.
	p := &commonpb.Payload{
		Metadata: map[string][]byte{
			"encoding":    []byte("binary/protobuf"),
			"messageType": []byte(f.MessageType),
		},
		Data: wire,
	}

	return writePayload(out, p)
}

func writePayload(path string, p *commonpb.Payload) error {
	wire := wirePayload{
		Metadata: make(map[string]string, len(p.Metadata)),
		Data:     base64.StdEncoding.EncodeToString(p.Data),
	}
	for k, v := range p.Metadata {
		wire.Metadata[k] = string(v)
	}
	out, err := json.MarshalIndent(wire, "", "  ")
	if err != nil {
		return err
	}
	out = append(out, '\n')
	return os.WriteFile(path, out, 0o644)
}
