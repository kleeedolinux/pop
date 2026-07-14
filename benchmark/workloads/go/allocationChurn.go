package main

import "fmt"

func main() {
	var total uint64
	for index := uint64(1); index <= 20_000; index++ {
		values := make([]uint64, 256)
		for slot := range values {
			values[slot] = index
		}
		total += values[0]
	}
	fmt.Println(total)
}
