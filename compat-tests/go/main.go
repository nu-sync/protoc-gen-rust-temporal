// compat-tests/go is the Go arm of the wire-format audit described in
// ../README.md. It walks fixtures/*.input.json, reconstructs the typed
// message, runs it through Temporal's standard ProtoPayloadConverter (which
// is what cludden/protoc-gen-go-temporal's generated clients use to encode
// workflow inputs), and writes <fixture>.go.payload.json next to the input.
//
// We hit the Temporal SDK converter directly rather than importing cludden's
// runtime because cludden's plugin does not override the converter chain —
// its codec package (pkg/codec) is a codec-server adapter for the Temporal
// UI, not the worker-side encoder. Auditing converter.NewProtoPayloadConverter
// audits cludden's wire format.

package main

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	jobsv1 "github.com/nu-sync/protoc-gen-rust-temporal/compat-tests/go/gen/jobs/v1"
	"go.temporal.io/sdk/converter"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/reflect/protoregistry"
	"google.golang.org/protobuf/types/known/emptypb"
)

type fixture struct {
	MessageType string          `json:"message_type"`
	Fields      json.RawMessage `json:"fields"`
}

type wirePayload struct {
	Metadata map[string]string `json:"metadata"`
	Data     string            `json:"data"`
}

// Force registration of the fixture types in the global proto registry.
var _ = (*jobsv1.JobInput)(nil)
var _ = (*emptypb.Empty)(nil)

func main() {
	if len(os.Args) < 2 || os.Args[1] != "generate" {
		fmt.Fprintln(os.Stderr, "usage: go run . generate")
		os.Exit(2)
	}

	matches, err := filepath.Glob("../fixtures/*.input.json")
	if err != nil {
		panic(err)
	}
	sort.Strings(matches)
	for _, in := range matches {
		out := strings.TrimSuffix(in, ".input.json") + ".go.payload.json"
		if err := process(in, out); err != nil {
			fmt.Fprintf(os.Stderr, "%s: %v\n", in, err)
			os.Exit(1)
		}
		fmt.Printf("wrote %s\n", out)
	}
}

var protoConv = converter.NewProtoPayloadConverter()

func process(in, out string) error {
	raw, err := os.ReadFile(in)
	if err != nil {
		return err
	}
	var f fixture
	if err := json.Unmarshal(raw, &f); err != nil {
		return err
	}

	msg, err := newMessage(f.MessageType)
	if err != nil {
		return err
	}
	// protojson handles proto3 conventions (snake_case field names, empty
	// defaults, nested messages) more faithfully than encoding/json. Treat
	// an empty fields object as a zero-value message.
	if len(f.Fields) > 0 && string(f.Fields) != "{}" && string(f.Fields) != "null" {
		if err := protojson.Unmarshal(f.Fields, msg); err != nil {
			return fmt.Errorf("protojson decode %s: %w", f.MessageType, err)
		}
	}

	payload, err := protoConv.ToPayload(msg)
	if err != nil {
		return fmt.Errorf("ToPayload %s: %w", f.MessageType, err)
	}

	return writePayload(out, payload)
}

func newMessage(fqn string) (proto.Message, error) {
	mt, err := protoregistry.GlobalTypes.FindMessageByName(protoreflect.FullName(fqn))
	if err != nil {
		return nil, fmt.Errorf("unknown message type %q: %w", fqn, err)
	}
	return mt.New().Interface(), nil
}

func writePayload(path string, p interface {
	GetData() []byte
	GetMetadata() map[string][]byte
}) error {
	wire := wirePayload{
		Metadata: make(map[string]string),
		Data:     base64.StdEncoding.EncodeToString(p.GetData()),
	}
	for k, v := range p.GetMetadata() {
		wire.Metadata[k] = string(v)
	}
	out, err := json.MarshalIndent(wire, "", "  ")
	if err != nil {
		return err
	}
	out = append(out, '\n')
	return os.WriteFile(path, out, 0o644)
}
