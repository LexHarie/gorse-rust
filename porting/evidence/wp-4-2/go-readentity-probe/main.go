package main

import (
	"bufio"
	"bytes"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"net/http/httptest"
	"os"
	restful "github.com/emicklei/go-restful/v3"
	"probe/data"
	"probe/server"
)

type caseIn struct {
	Target string `json:"target"`
	CT     string `json:"ct"`
	Body   string `json:"body"`
	Hex    string `json:"hex"`
}

type caseOut struct {
	Target string `json:"target"`
	CT     string `json:"ct"`
	Body   string `json:"body"`
	Hex    string `json:"hex"`
	Err    *string `json:"err"`
	Value  *string `json:"value"`
}

func decode(target string, req *restful.Request) (any, error) {
	switch target {
	case "user":
		var v data.User
		err := req.ReadEntity(&v)
		return v, err
	case "users":
		var v []data.User
		err := req.ReadEntity(&v)
		return v, err
	case "user-patch":
		var v data.UserPatch
		err := req.ReadEntity(&v)
		return v, err
	case "item":
		var v server.Item
		err := req.ReadEntity(&v)
		return v, err
	case "items":
		var v []server.Item
		err := req.ReadEntity(&v)
		return v, err
	case "item-patch":
		var v data.ItemPatch
		err := req.ReadEntity(&v)
		return v, err
	case "feedback":
		var v []server.Feedback
		err := req.ReadEntity(&v)
		return v, err
	}
	panic("unknown target " + target)
}

func main() {
	sc := bufio.NewScanner(os.Stdin)
	sc.Buffer(make([]byte, 1<<20), 1<<24)
	enc := json.NewEncoder(os.Stdout)
	for sc.Scan() {
		var in caseIn
		if err := json.Unmarshal([]byte(sc.Text()), &in); err != nil {
			panic(err)
		}
		raw := []byte(in.Body)
		if in.Hex != "" {
			var err error
			raw, err = hex.DecodeString(in.Hex)
			if err != nil {
				panic(err)
			}
			in.Body = string(raw)
		}
		httpReq := httptest.NewRequest("POST", "/", bytes.NewReader(raw))
		if in.CT != "<none>" {
			httpReq.Header.Set("Content-Type", in.CT)
		}
		v, err := decode(in.Target, restful.NewRequest(httpReq))
		out := caseOut{Target: in.Target, CT: in.CT, Body: in.Body, Hex: in.Hex}
		if err != nil {
			s := err.Error()
			out.Err = &s
		} else {
			b, merr := json.Marshal(v)
			s := string(b)
			if merr != nil {
				s = "MARSHAL ERROR: " + merr.Error()
			}
			out.Value = &s
		}
		if err := enc.Encode(out); err != nil {
			panic(err)
		}
	}
	fmt.Fprint(os.Stderr, "")
}
