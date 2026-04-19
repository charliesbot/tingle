// Go fixture. Exercises function + method + type captures.
// Also has imports in both grouped and single-import form.

package main

import (
	"fmt"
	"os"
)

import "strings"

type Server struct {
	Addr string
}

func (s *Server) Listen() error {
	fmt.Println("listening on", s.Addr)
	return nil
}

func main() {
	s := &Server{Addr: os.Args[1]}
	_ = strings.ToLower("FOO")
	s.Listen()
}
