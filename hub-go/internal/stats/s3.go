package stats

import (
	"bytes"
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"sort"
	"strings"
	"time"

	"github.com/muthuishere/klark/hub-go/internal/config"
)

// S3Sink puts and deletes objects with a small AWS Signature V4 client (no SDK, so the binary stays small and
// static). It works with AWS S3 and S3-compatible servers (MinIO, Cloudflare R2, ...).
type S3Sink struct {
	cfg    config.S3Config
	prefix string
	hc     *http.Client
	now    func() time.Time
}

// NormPrefix trims leading slashes and makes a non-empty prefix end with one.
func NormPrefix(p string) string {
	p = strings.TrimLeft(strings.TrimSpace(p), "/")
	if p != "" && !strings.HasSuffix(p, "/") {
		p += "/"
	}
	return p
}

func NewS3Sink(c config.S3Config, prefix string) (*S3Sink, error) {
	if strings.TrimSpace(c.Bucket) == "" {
		return nil, errors.New("s3: the bucket is empty")
	}
	if c.AccessKeyID == "" || c.SecretAccessKey == "" {
		return nil, errors.New("s3: the access key id and the secret access key are required")
	}
	if c.Region == "" {
		c.Region = "us-east-1"
	}
	if c.Endpoint != "" {
		u, err := url.Parse(c.Endpoint)
		if err != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" {
			return nil, errors.New("s3: the endpoint must be a URL like https://host:9000")
		}
	}
	return &S3Sink{cfg: c, prefix: NormPrefix(prefix), hc: &http.Client{Timeout: 60 * time.Second}, now: time.Now}, nil
}

func uriEncode(s string) string {
	var b strings.Builder
	for i := 0; i < len(s); i++ {
		c := s[i]
		if c >= 'A' && c <= 'Z' || c >= 'a' && c <= 'z' || c >= '0' && c <= '9' || c == '-' || c == '_' || c == '.' || c == '~' || c == '/' {
			b.WriteByte(c)
		} else {
			fmt.Fprintf(&b, "%%%02X", c)
		}
	}
	return b.String()
}

// target is the URL and canonical path of a key.
func (s *S3Sink) target(key string) (rawURL, path string) {
	full := uriEncode(s.prefix + key)
	if key == "\x00bucket" {
		full = ""
	}
	scheme, host := "https", ""
	if s.cfg.Endpoint != "" {
		u, _ := url.Parse(s.cfg.Endpoint)
		scheme, host = u.Scheme, u.Host
		if s.cfg.PathStyle {
			path = "/" + s.cfg.Bucket + "/" + full
		} else {
			host = s.cfg.Bucket + "." + host
			path = "/" + full
		}
	} else if s.cfg.PathStyle {
		host = "s3." + s.cfg.Region + ".amazonaws.com"
		path = "/" + s.cfg.Bucket + "/" + full
	} else {
		host = s.cfg.Bucket + ".s3." + s.cfg.Region + ".amazonaws.com"
		path = "/" + full
	}
	return scheme + "://" + host + path, path
}

func hm(key []byte, data string) []byte {
	h := hmac.New(sha256.New, key)
	h.Write([]byte(data))
	return h.Sum(nil)
}

func (s *S3Sink) do(ctx context.Context, method, key string, body []byte, contentType string) ([]byte, error) {
	return s.doQ(ctx, method, key, "", body, contentType)
}

func (s *S3Sink) doQ(ctx context.Context, method, key, query string, body []byte, contentType string) ([]byte, error) {
	rawURL, path := s.target(key)
	if query != "" {
		rawURL += "?" + query
	}
	req, err := http.NewRequestWithContext(ctx, method, rawURL, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	now := s.now().UTC()
	date, stamp := now.Format("20060102"), now.Format("20060102T150405Z")
	sum := sha256.Sum256(body)
	payload := hex.EncodeToString(sum[:])
	req.Header.Set("x-amz-date", stamp)
	req.Header.Set("x-amz-content-sha256", payload)
	if contentType != "" {
		req.Header.Set("Content-Type", contentType)
	}
	host := req.URL.Host
	canon := strings.Join([]string{method, path, query, "host:" + host + "\nx-amz-content-sha256:" + payload + "\nx-amz-date:" + stamp + "\n",
		"host;x-amz-content-sha256;x-amz-date", payload}, "\n")
	scope := date + "/" + s.cfg.Region + "/s3/aws4_request"
	csum := sha256.Sum256([]byte(canon))
	toSign := "AWS4-HMAC-SHA256\n" + stamp + "\n" + scope + "\n" + hex.EncodeToString(csum[:])
	k := hm(hm(hm(hm([]byte("AWS4"+s.cfg.SecretAccessKey), date), s.cfg.Region), "s3"), "aws4_request")
	sig := hex.EncodeToString(hm(k, toSign))
	req.Header.Set("Authorization", "AWS4-HMAC-SHA256 Credential="+s.cfg.AccessKeyID+"/"+scope+
		", SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature="+sig)
	resp, err := s.hc.Do(req)
	if err != nil {
		return nil, fmt.Errorf("s3 %s: %w", method, scrub(err, s.cfg.SecretAccessKey))
	}
	defer resp.Body.Close()
	if resp.StatusCode/100 == 2 {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, 32<<20))
		return b, nil
	}
	if resp.StatusCode == http.StatusNotFound && method == http.MethodGet {
		return nil, ErrNotFound
	}
	msg, _ := io.ReadAll(io.LimitReader(resp.Body, 512))
	return nil, fmt.Errorf("s3 %s %s: %s %s", method, key, resp.Status, strings.Join(strings.Fields(string(msg)), " "))
}

func scrub(err error, secret string) error {
	if secret != "" && strings.Contains(err.Error(), secret) {
		return errors.New(strings.ReplaceAll(err.Error(), secret, "***"))
	}
	return err
}

func (s *S3Sink) Put(ctx context.Context, key string, data []byte) error {
	_, err := s.do(ctx, http.MethodPut, key, data, "application/gzip")
	return err
}

// Get reads an object; ErrNotFound for a 404.
func (s *S3Sink) Get(ctx context.Context, key string) ([]byte, error) {
	return s.do(ctx, http.MethodGet, key, nil, "")
}

func (s *S3Sink) Delete(ctx context.Context, key string) error {
	_, err := s.do(ctx, http.MethodDelete, key, nil, "")
	return err
}

// List returns the keys (without the sink's prefix) under prefix: ListObjectsV2, following continuation tokens.
func (s *S3Sink) List(ctx context.Context, prefix string) ([]string, error) {
	var out []string
	token := ""
	for {
		q := "list-type=2&prefix=" + uriEncodeQ(s.prefix+prefix)
		if token != "" {
			q += "&continuation-token=" + uriEncodeQ(token)
		}
		// the query parameters must be sorted by name in the canonical request
		if token != "" {
			q = "continuation-token=" + uriEncodeQ(token) + "&list-type=2&prefix=" + uriEncodeQ(s.prefix+prefix)
		}
		b, err := s.doBucket(ctx, q)
		if err != nil {
			return nil, err
		}
		var res struct {
			Contents  []struct{ Key string } `xml:"Contents"`
			Truncated bool                   `xml:"IsTruncated"`
			Next      string                 `xml:"NextContinuationToken"`
		}
		if err := xml.Unmarshal(b, &res); err != nil {
			return nil, fmt.Errorf("s3 list: %w", err)
		}
		for _, c := range res.Contents {
			out = append(out, strings.TrimPrefix(c.Key, s.prefix))
		}
		if !res.Truncated || res.Next == "" {
			sort.Strings(out)
			return out, nil
		}
		token = res.Next
	}
}

func uriEncodeQ(v string) string { return strings.ReplaceAll(uriEncode(v), "/", "%2F") }

// doBucket is a GET on the bucket itself (no key) with a query.
func (s *S3Sink) doBucket(ctx context.Context, query string) ([]byte, error) {
	return s.doQ(ctx, http.MethodGet, "\x00bucket", query, nil, "")
}
