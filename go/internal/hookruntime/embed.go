package hookruntime

import _ "embed"

//go:embed runner.mts
var runner []byte

//go:embed bridge.mts
var bridge []byte

//go:embed process.mts
var process []byte

// File is one embedded runtime module.
type File struct {
	Name string
	Data []byte
}

// Files returns fresh copies of the runtime modules in installation order.
func Files() []File {
	return []File{
		{Name: "runner.mts", Data: append([]byte{}, runner...)},
		{Name: "bridge.mts", Data: append([]byte{}, bridge...)},
		{Name: "process.mts", Data: append([]byte{}, process...)},
	}
}
