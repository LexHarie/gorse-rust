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

// Command tlsgolden writes the certificates and the verdicts that the Rust
// port of common/util/tls.go (crates/gorse-common/src/util/tls.rs) is checked
// against:
//
//	go run ./contracts/tools/tlsgolden \
//	  -dir crates/gorse-common/tests/data/tls \
//	  -out crates/gorse-common/tests/data/tls_golden.json
//
// -dir receives certificate authorities, key pairs and broken files. "ca" and
// "node" are made the way github.com/madflojo/testcerts makes them for
// Gorse's TestSSL (P-521 keys, PKCS#8, both extended key usages, the same
// subject and serial number for every certificate); the others vary one thing
// each. Certificates are valid from 2000 to 9999 unless a case needs them
// expired or not yet valid, so the committed files keep working; keys are
// random, so every run rewrites every file.
//
// -out receives one case per pair of server and client settings: the error,
// if any, of util.NewServerCreds and util.NewClientCreds on those files and,
// when both succeed, what a handshake between them over loopback TCP gives:
// whether a ping and a pong went through, which side rejected the other, and
// the negotiated ALPN protocol. Go's error texts are kept for reading.
//
// With -interop <program>, the tool runs every case of the existing files
// between Go and the Rust peer instead (crates/gorse-common/examples/tls_peer.rs,
// built by `cargo build -p gorse-common --example tls_peer`): a Go server
// against a Rust client and a Rust server against a Go client. It prints one
// line per run and fails if an outcome differs from the Go-to-Go one.
package main

import (
	"bufio"
	"bytes"
	"context"
	"crypto"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/rsa"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/json"
	"encoding/pem"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"math/big"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"time"

	"github.com/gorse-io/gorse/common/util"
	"google.golang.org/grpc/credentials"
)

const organization = "Never Use this Certificate in Production Inc."

var (
	start  = time.Date(2000, 1, 1, 0, 0, 0, 0, time.UTC)
	end    = time.Date(9999, 12, 31, 23, 59, 59, 0, time.UTC)
	past   = time.Date(2001, 1, 1, 0, 0, 0, 0, time.UTC)
	future = time.Date(9000, 1, 1, 0, 0, 0, 0, time.UTC)
)

// Side names the files of one peer, relative to -dir.
type Side struct {
	CA   string `json:"ca"`
	Cert string `json:"cert"`
	Key  string `json:"key"`
}

// Client modes.
const (
	// Creds uses util.NewClientCreds.
	Creds = "creds"
	// NoCert is a TLS client that verifies the server against the CA file
	// but presents no certificate.
	NoCert = "no-cert"
	// Plaintext sends gRPC's HTTP/2 preface without TLS, as
	// grpc.WithInsecure() does.
	Plaintext = "plaintext"
)

// Case is one golden case.
type Case struct {
	Name       string `json:"name"`
	Server     Side   `json:"server"`
	Client     Side   `json:"client"`
	ClientMode string `json:"client_mode"`
	// Errors of util.NewServerCreds and util.NewClientCreds; no handshake
	// is attempted when either fails.
	ServerConfigError string `json:"server_config_error"`
	ClientConfigError string `json:"client_config_error"`
	// The handshake: whether the ping and the pong went through, which side
	// failed of its own accord ("server", "client", or "" when none), and
	// the ALPN protocol both sides agreed on.
	Established bool   `json:"established"`
	RejectedBy  string `json:"rejected_by"`
	ALPN        string `json:"alpn"`
	// Go's errors on each side, for reading only.
	ServerError string `json:"server_error"`
	ClientError string `json:"client_error"`
}

// Golden is the -out file.
type Golden struct {
	GoVersion string `json:"go_version"`
	Cases     []Case `json:"cases"`
}

// Outcome is what one peer saw, in the form the Rust peer prints it too.
type Outcome struct {
	ConfigError    string `json:"config_error"`
	HandshakeError string `json:"handshake_error"`
	// Local is true when this side failed of its own accord, not because the
	// peer sent an alert or closed the connection.
	Local     bool   `json:"local"`
	Exchanged bool   `json:"exchanged"`
	ALPN      string `json:"alpn"`
}

func main() {
	dir := flag.String("dir", "crates/gorse-common/tests/data/tls", "directory of the certificate files")
	out := flag.String("out", "crates/gorse-common/tests/data/tls_golden.json", "golden JSON file")
	interop := flag.String("interop", "", "Rust peer program; runs the existing cases between Go and Rust")
	flag.Parse()
	if *interop != "" {
		if err := runInterop(*dir, *out, *interop); err != nil {
			log.Fatal(err)
		}
		return
	}
	if err := os.MkdirAll(*dir, 0o755); err != nil {
		log.Fatal(err)
	}
	if err := writeFixtures(*dir); err != nil {
		log.Fatal(err)
	}
	golden := Golden{GoVersion: runtime.Version()}
	for _, c := range cases() {
		golden.Cases = append(golden.Cases, run(*dir, c))
	}
	data, err := json.MarshalIndent(golden, "", "  ")
	if err != nil {
		log.Fatal(err)
	}
	if err := os.WriteFile(*out, append(data, '\n'), 0o644); err != nil {
		log.Fatal(err)
	}
}

// authority is a CA certificate with its key.
type authority struct {
	cert *x509.Certificate
	key  crypto.Signer
}

func p521() crypto.Signer {
	key, err := ecdsa.GenerateKey(elliptic.P521(), rand.Reader)
	if err != nil {
		log.Fatal(err)
	}
	return key
}

func p256() crypto.Signer {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		log.Fatal(err)
	}
	return key
}

// caTemplate is testcerts.NewCA's template with a lasting validity period.
func caTemplate() *x509.Certificate {
	return &x509.Certificate{
		Subject:               pkix.Name{Organization: []string{organization}},
		SerialNumber:          big.NewInt(42),
		NotBefore:             start,
		NotAfter:              end,
		IsCA:                  true,
		ExtKeyUsage:           []x509.ExtKeyUsage{x509.ExtKeyUsageClientAuth, x509.ExtKeyUsageServerAuth},
		KeyUsage:              x509.KeyUsageDigitalSignature | x509.KeyUsageCertSign | x509.KeyUsageCRLSign,
		BasicConstraintsValid: true,
	}
}

// leafTemplate is testcerts.NewKeyPair's template (no domains given) with a
// lasting validity period.
func leafTemplate() *x509.Certificate {
	return &x509.Certificate{
		Subject:      pkix.Name{Organization: []string{organization}},
		DNSNames:     []string{"localhost"},
		IPAddresses:  []net.IP{net.ParseIP("127.0.0.1"), net.ParseIP("::1")},
		SerialNumber: big.NewInt(42),
		NotBefore:    start,
		NotAfter:     end,
		ExtKeyUsage:  []x509.ExtKeyUsage{x509.ExtKeyUsageClientAuth, x509.ExtKeyUsageServerAuth},
		KeyUsage:     x509.KeyUsageDigitalSignature,
	}
}

// sign creates a certificate for key from template, issued by parent (or
// self-signed when parent is nil).
func sign(template *x509.Certificate, key crypto.Signer, parent *authority) *x509.Certificate {
	issuer, issuerKey := template, key
	if parent != nil {
		issuer, issuerKey = parent.cert, parent.key
	}
	der, err := x509.CreateCertificate(rand.Reader, template, issuer, key.Public(), issuerKey)
	if err != nil {
		log.Fatal(err)
	}
	cert, err := x509.ParseCertificate(der)
	if err != nil {
		log.Fatal(err)
	}
	return cert
}

func newCA(key crypto.Signer, parent *authority, edit func(*x509.Certificate)) *authority {
	template := caTemplate()
	if edit != nil {
		edit(template)
	}
	return &authority{cert: sign(template, key, parent), key: key}
}

func certBlock(c *x509.Certificate) []byte {
	return pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: c.Raw})
}

// Key encodings Go's tls.LoadX509KeyPair reads.
const (
	pkcs8 = iota
	sec1
	pkcs1
)

func keyBlock(key crypto.Signer, encoding int) []byte {
	var block *pem.Block
	switch encoding {
	case pkcs8:
		der, err := x509.MarshalPKCS8PrivateKey(key)
		if err != nil {
			log.Fatal(err)
		}
		block = &pem.Block{Type: "PRIVATE KEY", Bytes: der}
	case sec1:
		der, err := x509.MarshalECPrivateKey(key.(*ecdsa.PrivateKey))
		if err != nil {
			log.Fatal(err)
		}
		block = &pem.Block{Type: "EC PRIVATE KEY", Bytes: der}
	case pkcs1:
		block = &pem.Block{Type: "RSA PRIVATE KEY", Bytes: x509.MarshalPKCS1PrivateKey(key.(*rsa.PrivateKey))}
	}
	return pem.EncodeToMemory(block)
}

func writeFixtures(dir string) error {
	write := func(name string, parts ...[]byte) {
		if err := os.WriteFile(filepath.Join(dir, name), bytes.Join(parts, nil), 0o644); err != nil {
			log.Fatal(err)
		}
	}
	named := func(cn string) func(*x509.Certificate) {
		return func(t *x509.Certificate) { t.Subject.CommonName = cn }
	}

	// Certificate authorities. ca and ca2 have the same subject, as two
	// testcerts.NewCA() do.
	ca := newCA(p521(), nil, nil)
	ca2 := newCA(p521(), nil, nil)
	caExpired := newCA(p256(), nil, func(t *x509.Certificate) {
		named("expired CA")(t)
		t.NotAfter = past
	})
	caFuture := newCA(p256(), nil, func(t *x509.Certificate) {
		named("future CA")(t)
		t.NotBefore = future
	})
	caServerAuth := newCA(p256(), nil, func(t *x509.Certificate) {
		named("server-auth CA")(t)
		t.ExtKeyUsage = []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth}
	})
	caPathLen0 := newCA(p256(), nil, func(t *x509.Certificate) {
		named("path length 0 CA")(t)
		t.MaxPathLen = 0
		t.MaxPathLenZero = true
	})
	inter := newCA(p256(), ca, named("intermediate"))
	interPathLen0 := newCA(p256(), caPathLen0, named("intermediate under path length 0"))
	interNoCertSign := newCA(p256(), ca, func(t *x509.Certificate) {
		named("intermediate without certSign")(t)
		t.KeyUsage = x509.KeyUsageDigitalSignature
	})
	for name, a := range map[string]*authority{
		"ca.pem": ca, "ca2.pem": ca2, "ca-expired.pem": caExpired, "ca-future.pem": caFuture,
		"ca-server-auth.pem": caServerAuth, "ca-pathlen0.pem": caPathLen0,
	} {
		write(name, certBlock(a.cert))
	}

	// Key pairs: <name>.pem holds the chain, <name>.key the key.
	pair := func(name string, key crypto.Signer, encoding int, parent *authority, edit func(*x509.Certificate), chain ...*authority) {
		template := leafTemplate()
		if edit != nil {
			edit(template)
		}
		blocks := [][]byte{certBlock(sign(template, key, parent))}
		for _, c := range chain {
			blocks = append(blocks, certBlock(c.cert))
		}
		write(name+".pem", blocks...)
		write(name+".key", keyBlock(key, encoding))
	}
	rsaKey := func() crypto.Signer {
		key, err := rsa.GenerateKey(rand.Reader, 2048)
		if err != nil {
			log.Fatal(err)
		}
		return key
	}
	_, edKey, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return err
	}
	nodeKey := p521()
	pair("node", nodeKey, pkcs8, ca, nil)
	pair("node2", p521(), pkcs8, ca2, nil)
	p256Key := p256()
	pair("p256-sec1", p256Key, sec1, ca, nil)
	p384Key, err := ecdsa.GenerateKey(elliptic.P384(), rand.Reader)
	if err != nil {
		return err
	}
	pair("p384", p384Key, pkcs8, ca, nil)
	pair("rsa-pkcs1", rsaKey(), pkcs1, ca, nil)
	pair("rsa-pkcs8", rsaKey(), pkcs8, ca, nil)
	pair("ed25519", edKey, pkcs8, ca, nil)
	pair("expired", p521(), pkcs8, ca, func(t *x509.Certificate) { t.NotAfter = past })
	pair("future", p521(), pkcs8, ca, func(t *x509.Certificate) { t.NotBefore = future })
	pair("server-auth", p521(), pkcs8, ca, func(t *x509.Certificate) {
		t.ExtKeyUsage = []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth}
	})
	pair("client-auth", p521(), pkcs8, ca, func(t *x509.Certificate) {
		t.ExtKeyUsage = []x509.ExtKeyUsage{x509.ExtKeyUsageClientAuth}
	})
	pair("no-eku", p521(), pkcs8, ca, func(t *x509.Certificate) { t.ExtKeyUsage = nil })
	pair("any-eku", p521(), pkcs8, ca, func(t *x509.Certificate) {
		t.ExtKeyUsage = []x509.ExtKeyUsage{x509.ExtKeyUsageAny}
	})
	pair("other-host", p521(), pkcs8, ca, func(t *x509.Certificate) {
		t.DNSNames = []string{"elsewhere.example"}
		t.IPAddresses = nil
	})
	pair("chain", p256(), pkcs8, inter, nil, inter)
	pair("chain-missing", p256(), pkcs8, inter, nil)
	pair("chain-no-certsign", p256(), pkcs8, interNoCertSign, nil, interNoCertSign)
	pair("by-expired-ca", p256(), pkcs8, caExpired, nil)
	pair("by-future-ca", p256(), pkcs8, caFuture, nil)
	pair("by-server-auth-ca", p256(), pkcs8, caServerAuth, nil)
	pair("by-pathlen0-ca", p256(), pkcs8, interPathLen0, nil, interPathLen0)
	pair("self-signed", p256(), pkcs8, nil, nil)

	// CA files Go reads in other ways, and broken key pairs.
	nodePEM, err := os.ReadFile(filepath.Join(dir, "node.pem"))
	if err != nil {
		return err
	}
	write("ca-bundle.pem", []byte("Two authorities, with a key and this text around them.\n"),
		keyBlock(nodeKey, pkcs8), certBlock(ca2.cert), certBlock(ca.cert))
	write("ca-none.pem", keyBlock(nodeKey, pkcs8))
	write("ca-corrupt.pem", pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: []byte("not DER")}))
	write("cert-none.pem", keyBlock(nodeKey, pkcs8))
	write("key-none.key", nodePEM)
	return nil
}

func side(ca, name string) Side {
	return Side{CA: ca, Cert: name + ".pem", Key: name + ".key"}
}

func cases() []Case {
	node := side("ca.pem", "node")
	var cs []Case
	add := func(name string, server Side, client Side, mode string) {
		cs = append(cs, Case{Name: name, Server: server, Client: client, ClientMode: mode})
	}
	// master/rpc_test.go TestSSL.
	add("TestSSL/success", node, node, Creds)
	add("TestSSL/insecure", node, Side{}, Plaintext)
	add("TestSSL/certificate mismatch", node, side("ca2.pem", "node2"), Creds)
	// One side's key pair varies.
	for _, name := range []string{
		"p256-sec1", "p384", "rsa-pkcs1", "rsa-pkcs8", "ed25519", "expired", "future", "server-auth",
		"client-auth", "no-eku", "any-eku", "other-host", "chain", "chain-missing", "chain-no-certsign",
		"self-signed",
	} {
		add("server key pair/"+name, side("ca.pem", name), node, Creds)
		add("client key pair/"+name, node, side("ca.pem", name), Creds)
	}
	// The CA file that verifies the peer varies.
	for _, c := range []struct{ ca, peer string }{
		{"ca-expired.pem", "by-expired-ca"},
		{"ca-future.pem", "by-future-ca"},
		{"ca-server-auth.pem", "by-server-auth-ca"},
		{"ca-pathlen0.pem", "by-pathlen0-ca"},
		{"ca-bundle.pem", "node2"},
	} {
		name := strings.TrimSuffix(strings.TrimPrefix(c.ca, "ca-"), ".pem")
		add("server CA/"+name, side(c.ca, "node"), side("ca.pem", c.peer), Creds)
		add("client CA/"+name, side("ca.pem", c.peer), side(c.ca, "node"), Creds)
	}
	// Files util rejects.
	for _, c := range []struct{ name, ca, cert, key string }{
		{"CA file missing", "missing.pem", "node.pem", "node.key"},
		{"CA file without certificates", "ca-none.pem", "node.pem", "node.key"},
		{"CA file corrupt", "ca-corrupt.pem", "node.pem", "node.key"},
		{"certificate file missing", "ca.pem", "missing.pem", "node.key"},
		{"key file missing", "ca.pem", "node.pem", "missing.key"},
		{"certificate file without certificates", "ca.pem", "cert-none.pem", "node.key"},
		{"key file without keys", "ca.pem", "node.pem", "key-none.key"},
		{"key not matching certificate", "ca.pem", "node.pem", "p256-sec1.key"},
	} {
		broken := Side{CA: c.ca, Cert: c.cert, Key: c.key}
		add("server files/"+c.name, broken, node, Creds)
		add("client files/"+c.name, node, broken, Creds)
	}
	// A client with no certificate.
	add("client without certificate", node, Side{CA: "ca.pem"}, NoCert)
	return cs
}

func config(dir string, s Side) *util.TLSConfig {
	path := func(name string) string { return filepath.Join(dir, name) }
	return &util.TLSConfig{SSLCA: path(s.CA), SSLCert: path(s.Cert), SSLKey: path(s.Key)}
}

func run(dir string, c Case) Case {
	serverCreds, err := util.NewServerCreds(config(dir, c.Server))
	if err != nil {
		c.ServerConfigError = err.Error()
	}
	var clientCreds credentials.TransportCredentials
	if c.ClientMode == Creds {
		clientCreds, err = util.NewClientCreds(config(dir, c.Client))
		if err != nil {
			c.ClientConfigError = err.Error()
		}
	}
	if c.ServerConfigError != "" || c.ClientConfigError != "" {
		return c
	}
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		log.Fatal(err)
	}
	served := make(chan Outcome)
	go func() { served <- serve(listener, serverCreds) }()
	client := dial(listener.Addr().String(), dir, c, clientCreds)
	server := <-served
	return combine(c, server, client)
}

func combine(c Case, server, client Outcome) Case {
	c.ServerError, c.ClientError = server.HandshakeError, client.HandshakeError
	c.Established = server.Exchanged && client.Exchanged
	switch {
	case server.Local && client.Local:
		c.RejectedBy = "both"
	case server.Local:
		c.RejectedBy = "server"
	case client.Local:
		c.RejectedBy = "client"
	}
	if c.Established {
		c.ALPN = client.ALPN
	}
	return c
}

// local reports whether err is this side's own failure rather than the
// peer's alert or the connection closing.
func local(err error) bool {
	if err == nil {
		return false
	}
	text := err.Error()
	return !(errors.Is(err, io.EOF) || errors.Is(err, io.ErrUnexpectedEOF) || errors.Is(err, syscall.ECONNRESET) ||
		errors.Is(err, syscall.EPIPE) || errors.Is(err, net.ErrClosed) || strings.Contains(text, "remote error: "))
}

func fail(o Outcome, err error) Outcome {
	o.HandshakeError = err.Error()
	o.Local = local(err)
	return o
}

const deadline = 10 * time.Second

// serve accepts one connection and runs util.NewServerCreds' handshake on it,
// then reads "ping\n" and answers "pong\n".
func serve(listener net.Listener, creds credentials.TransportCredentials) Outcome {
	defer func() { _ = listener.Close() }()
	raw, err := listener.Accept()
	if err != nil {
		log.Fatal(err)
	}
	defer func() { _ = raw.Close() }()
	_ = raw.SetDeadline(time.Now().Add(deadline))
	conn, info, err := creds.ServerHandshake(raw)
	if err != nil {
		return fail(Outcome{}, err)
	}
	o := Outcome{ALPN: info.(credentials.TLSInfo).State.NegotiatedProtocol}
	buf := make([]byte, 5)
	if _, err := io.ReadFull(conn, buf); err != nil {
		return fail(o, err)
	}
	if string(buf) != "ping\n" {
		return fail(o, fmt.Errorf("unexpected %q", buf))
	}
	if _, err := conn.Write([]byte("pong\n")); err != nil {
		return fail(o, err)
	}
	o.Exchanged = true
	_ = conn.Close()
	return o
}

// dial connects to addr as the case's client: with util.NewClientCreds, as
// a TLS client without a certificate, or in plaintext.
func dial(addr, dir string, c Case, creds credentials.TransportCredentials) Outcome {
	raw, err := net.DialTimeout("tcp", addr, deadline)
	if err != nil {
		log.Fatal(err)
	}
	defer func() { _ = raw.Close() }()
	_ = raw.SetDeadline(time.Now().Add(deadline))
	var conn net.Conn
	var o Outcome
	switch c.ClientMode {
	case Creds:
		ctx, cancel := context.WithTimeout(context.Background(), deadline)
		defer cancel()
		// gRPC passes the dial target as the authority.
		var info credentials.AuthInfo
		conn, info, err = creds.ClientHandshake(ctx, addr, raw)
		if err != nil {
			return fail(o, err)
		}
		o.ALPN = info.(credentials.TLSInfo).State.NegotiatedProtocol
	case NoCert:
		pool := x509.NewCertPool()
		data, err := os.ReadFile(filepath.Join(dir, c.Client.CA))
		if err != nil || !pool.AppendCertsFromPEM(data) {
			log.Fatalf("read %s: %v", c.Client.CA, err)
		}
		tc := tls.Client(raw, &tls.Config{RootCAs: pool, ServerName: "localhost", NextProtos: []string{"h2"}})
		if err := tc.Handshake(); err != nil {
			return fail(o, err)
		}
		conn, o.ALPN = tc, tc.ConnectionState().NegotiatedProtocol
	case Plaintext:
		if _, err := raw.Write([]byte("PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")); err != nil {
			return Outcome{HandshakeError: err.Error()}
		}
		_, err := io.ReadAll(raw)
		if err == nil {
			err = io.EOF
		}
		// A plaintext client has no handshake of its own to fail.
		return Outcome{HandshakeError: err.Error()}
	}
	if _, err := conn.Write([]byte("ping\n")); err != nil {
		return fail(o, err)
	}
	buf := make([]byte, 5)
	if _, err := io.ReadFull(conn, buf); err != nil {
		return fail(o, err)
	}
	if string(buf) != "pong\n" {
		return fail(o, fmt.Errorf("unexpected %q", buf))
	}
	o.Exchanged = true
	return o
}

// runInterop runs every case of the golden file between Go and the Rust
// peer, in both directions.
func runInterop(dir, out, program string) error {
	data, err := os.ReadFile(out)
	if err != nil {
		return err
	}
	var golden Golden
	if err := json.Unmarshal(data, &golden); err != nil {
		return err
	}
	runs, mismatches := 0, 0
	for _, want := range golden.Cases {
		for _, rustServes := range []bool{false, true} {
			runs++
			got, err := interop(program, dir, want, rustServes)
			if err != nil {
				return err
			}
			same := got.ServerConfigError != "" == (want.ServerConfigError != "") &&
				got.ClientConfigError != "" == (want.ClientConfigError != "") &&
				got.Established == want.Established && got.RejectedBy == want.RejectedBy && got.ALPN == want.ALPN
			status := "ok"
			if !same {
				status = "MISMATCH"
				mismatches++
			}
			direction := "Go server, Rust client"
			if rustServes {
				direction = "Rust server, Go client"
			}
			fmt.Printf("%-8s %-22s %-52s config=%v/%v established=%v rejected_by=%q alpn=%q\n", status, direction,
				want.Name, got.ServerConfigError == "", got.ClientConfigError == "", got.Established, got.RejectedBy,
				got.ALPN)
			if !same {
				fmt.Printf("         server: %s\n         client: %s\n", got.ServerError, got.ClientError)
			}
		}
	}
	fmt.Printf("%d runs, %d differ from Go to Go\n", runs, mismatches)
	if mismatches > 0 {
		return fmt.Errorf("%d of %d runs differ from Go to Go", mismatches, runs)
	}
	return nil
}

// interop runs one case with the Rust peer on one side and Go on the other.
func interop(program, dir string, c Case, rustServes bool) (Case, error) {
	c.ServerConfigError, c.ClientConfigError = "", ""
	c.Established, c.RejectedBy, c.ALPN, c.ServerError, c.ClientError = false, "", "", "", ""
	if rustServes {
		check, err := rustPeer(program, "check-server", "", dir, Creds, c.Server)
		if err != nil {
			return c, err
		}
		c.ServerConfigError = check.ConfigError
		var clientCreds credentials.TransportCredentials
		if c.ClientMode == Creds {
			if clientCreds, err = util.NewClientCreds(config(dir, c.Client)); err != nil {
				c.ClientConfigError = err.Error()
			}
		}
		if c.ServerConfigError != "" || c.ClientConfigError != "" {
			return c, nil
		}
		server, err := startRustServer(program, dir, c.Server)
		if err != nil {
			return c, err
		}
		client := dial(server.addr, dir, c, clientCreds)
		outcome, err := server.finish()
		if err != nil {
			return c, err
		}
		return combine(c, outcome, client), nil
	}
	check, err := rustPeer(program, "check-client", "", dir, c.ClientMode, c.Client)
	if err != nil {
		return c, err
	}
	c.ClientConfigError = check.ConfigError
	serverCreds, err := util.NewServerCreds(config(dir, c.Server))
	if err != nil {
		c.ServerConfigError = err.Error()
	}
	if c.ServerConfigError != "" || c.ClientConfigError != "" {
		return c, nil
	}
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return c, err
	}
	served := make(chan Outcome, 1)
	go func() { served <- serve(listener, serverCreds) }()
	client, err := rustPeer(program, "client", listener.Addr().String(), dir, c.ClientMode, c.Client)
	if err != nil {
		return c, err
	}
	return combine(c, <-served, client), nil
}

func peerArgs(dir string, s Side) []string {
	path := func(name string) string {
		if name == "" {
			return ""
		}
		return filepath.Join(dir, name)
	}
	return []string{path(s.CA), path(s.Cert), path(s.Key)}
}

func rustPeer(program, role, addr, dir, mode string, s Side) (Outcome, error) {
	args := append([]string{role, addr, mode}, peerArgs(dir, s)...)
	cmd := exec.Command(program, args...)
	cmd.Stderr = os.Stderr
	data, err := cmd.Output()
	if err != nil {
		return Outcome{}, fmt.Errorf("%s %v: %w", program, args, err)
	}
	var o Outcome
	if err := json.Unmarshal(data, &o); err != nil {
		return Outcome{}, fmt.Errorf("%s %v: %q: %w", program, args, data, err)
	}
	return o, nil
}

// rustServer is a running Rust server peer: it prints its address, then,
// after one connection, its outcome.
type rustServer struct {
	cmd   *exec.Cmd
	lines *bufio.Scanner
	addr  string
}

func startRustServer(program, dir string, s Side) (*rustServer, error) {
	args := append([]string{"server", "127.0.0.1:0", Creds}, peerArgs(dir, s)...)
	cmd := exec.Command(program, args...)
	cmd.Stderr = os.Stderr
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	r := &rustServer{cmd: cmd, lines: bufio.NewScanner(stdout)}
	if !r.lines.Scan() {
		return nil, fmt.Errorf("%s %v: no address", program, args)
	}
	var first struct {
		Addr string `json:"addr"`
	}
	if err := json.Unmarshal(r.lines.Bytes(), &first); err != nil {
		return nil, err
	}
	r.addr = first.Addr
	return r, nil
}

// finish reads the server's outcome after its connection.
func (r *rustServer) finish() (Outcome, error) {
	if !r.lines.Scan() {
		return Outcome{}, errors.New("rust server: no outcome")
	}
	var o Outcome
	if err := json.Unmarshal(r.lines.Bytes(), &o); err != nil {
		return Outcome{}, err
	}
	return o, r.cmd.Wait()
}
