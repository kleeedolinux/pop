class Box
  attr_reader :value

  def initialize(value)
    @value = value
  end
end

values = Array.new(200_000) { |index| Box.new(index + 1) }
total = 0
index = 0
while index < values.length
  total += values[index].value
  index += 1
end
puts total
