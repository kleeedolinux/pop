package main

import "fmt"

func main() {
	var value uint64
	for index := uint64(1); index <= 50_000_000; index++ {
		value += index
	}
	fmt.Println(value)
}
