// Copyright 2026 gorse Project Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Command protogolden writes golden protobuf encodings produced by the Go
// protocol package, for the Rust port's round-trip tests.
//
// Every message in package protocol is filled deterministically: each scalar
// field gets a value derived from its field number, each repeated field gets two
// elements, each message field is filled recursively (to a fixed depth), and
// each optional field is set. The output maps the full message name to the hex
// of proto.MarshalOptions{Deterministic: true}.Marshal, plus the full method
// paths of every service.
package main

import (
	"encoding/hex"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"sort"

	_ "github.com/gorse-io/gorse/protocol"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"
	"google.golang.org/protobuf/reflect/protoregistry"
)

const maxDepth = 3

type golden struct {
	Messages map[string]string   `json:"messages"`
	Empty    map[string]string   `json:"empty"`
	Methods  map[string][]string `json:"methods"`
}

func main() {
	out := flag.String("out", "", "output JSON file")
	flag.Parse()
	g := golden{
		Messages: map[string]string{},
		Empty:    map[string]string{},
		Methods:  map[string][]string{},
	}
	protoregistry.GlobalFiles.RangeFilesByPackage("protocol", func(fd protoreflect.FileDescriptor) bool {
		messages := fd.Messages()
		for i := 0; i < messages.Len(); i++ {
			md := messages.Get(i)
			mt, err := protoregistry.GlobalTypes.FindMessageByName(md.FullName())
			if err != nil {
				panic(err)
			}
			msg := mt.New()
			fill(msg, 0)
			g.Messages[string(md.FullName())] = encode(msg.Interface())
			g.Empty[string(md.FullName())] = encode(mt.New().Interface())
		}
		services := fd.Services()
		for i := 0; i < services.Len(); i++ {
			sd := services.Get(i)
			methods := sd.Methods()
			var paths []string
			for j := 0; j < methods.Len(); j++ {
				m := methods.Get(j)
				kind := "unary"
				switch {
				case m.IsStreamingClient() && m.IsStreamingServer():
					kind = "bidi"
				case m.IsStreamingClient():
					kind = "client-stream"
				case m.IsStreamingServer():
					kind = "server-stream"
				}
				paths = append(paths, fmt.Sprintf("/%s/%s %s", sd.FullName(), m.Name(), kind))
			}
			sort.Strings(paths)
			g.Methods[string(sd.FullName())] = paths
		}
		return true
	})
	data, err := json.MarshalIndent(g, "", "  ")
	if err != nil {
		panic(err)
	}
	data = append(data, '\n')
	if err = os.WriteFile(*out, data, 0o644); err != nil {
		panic(err)
	}
}

func encode(m proto.Message) string {
	data, err := proto.MarshalOptions{Deterministic: true}.Marshal(m)
	if err != nil {
		panic(err)
	}
	return hex.EncodeToString(data)
}

func fill(msg protoreflect.Message, depth int) {
	fields := msg.Descriptor().Fields()
	for i := 0; i < fields.Len(); i++ {
		fd := fields.Get(i)
		if fd.Kind() == protoreflect.MessageKind && depth >= maxDepth {
			continue
		}
		switch {
		case fd.IsList():
			list := msg.Mutable(fd).List()
			for k := 0; k < 2; k++ {
				if fd.Kind() == protoreflect.MessageKind {
					elem := list.NewElement()
					fill(elem.Message(), depth+1)
					list.Append(elem)
				} else {
					list.Append(scalar(fd, k))
				}
			}
		case fd.IsMap():
			panic("maps are not used by the protocol package")
		case fd.Kind() == protoreflect.MessageKind:
			fill(msg.Mutable(fd).Message(), depth+1)
		default:
			msg.Set(fd, scalar(fd, 0))
		}
	}
}

// scalar returns a non-default value for field fd; k distinguishes list elements.
func scalar(fd protoreflect.FieldDescriptor, k int) protoreflect.Value {
	n := int64(fd.Number())*10 + int64(k) + 1
	switch fd.Kind() {
	case protoreflect.BoolKind:
		return protoreflect.ValueOfBool(true)
	case protoreflect.EnumKind:
		values := fd.Enum().Values()
		return protoreflect.ValueOfEnum(values.Get(values.Len() - 1).Number())
	case protoreflect.Int32Kind, protoreflect.Sint32Kind, protoreflect.Sfixed32Kind:
		return protoreflect.ValueOfInt32(int32(-n))
	case protoreflect.Int64Kind, protoreflect.Sint64Kind, protoreflect.Sfixed64Kind:
		return protoreflect.ValueOfInt64(-n * 1_000_000_007)
	case protoreflect.Uint32Kind, protoreflect.Fixed32Kind:
		return protoreflect.ValueOfUint32(uint32(n))
	case protoreflect.Uint64Kind, protoreflect.Fixed64Kind:
		return protoreflect.ValueOfUint64(uint64(n) * 1_000_000_007)
	case protoreflect.FloatKind:
		return protoreflect.ValueOfFloat32(float32(n) + 0.25)
	case protoreflect.DoubleKind:
		return protoreflect.ValueOfFloat64(float64(n) + 0.125)
	case protoreflect.StringKind:
		return protoreflect.ValueOfString(fmt.Sprintf("%s-%d-é", fd.Name(), n))
	case protoreflect.BytesKind:
		return protoreflect.ValueOfBytes([]byte(fmt.Sprintf("[%d,\"%s\"]", n, fd.Name())))
	default:
		panic(fmt.Sprintf("unexpected kind %v", fd.Kind()))
	}
}
