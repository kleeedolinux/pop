class BenchmarkBox
  getter value : Int64

  def initialize(@value : Int64)
  end
end

values = Array(BenchmarkBox).new(200_000) { |index| BenchmarkBox.new(index.to_i64 + 1) }
total = 0_i64
values.each { |value| total += value.value }
puts total
