# frozen_string_literal: true

load File.expand_path("../bin/benchmark", __dir__)

module BenchmarkAssertions
  module_function

  def assert(condition, message)
    raise message unless condition
  end

  def includes(value, expected)
    assert(value.include?(expected), "expected output to contain #{expected.inspect}")
  end
end

ids = PopBenchmark::REGISTRY.runtimes.map(&:id)
%w[ruby go c d csharp crystal poplang].each do |id|
  BenchmarkAssertions.assert(ids.include?(id), "benchmark matrix is missing #{id}")
end
%w[go c d csharp crystal poplang].each do |id|
  runtime = PopBenchmark::REGISTRY.runtimes.find { |candidate| candidate.id == id }
  BenchmarkAssertions.assert(runtime.builder, "#{id} must compile outside the timed region")
end

document = {
  "schemaVersion" => 2,
  "createdAt" => "2026-07-12T12:00:00Z",
  "samples" => 3,
  "warmups" => 1,
  "timing" => "execution only",
  "workloads" => [
    { "id" => "integerLoop", "name" => "Integer loop", "description" => "Bad </script> payload" }
  ],
  "results" => [
    {
      "runtime" => "poplang", "runtimeName" => "Pop Lang", "executionModel" => "ahead of time",
      "workload" => "integerLoop", "workloadName" => "Integer loop", "samplesSeconds" => [0.02, 0.03, 0.04],
      "medianSeconds" => 0.03, "minimumSeconds" => 0.02, "comparable" => true
    },
    {
      "runtime" => "ruby", "runtimeName" => "Ruby", "executionModel" => "interpreted",
      "workload" => "integerLoop", "workloadName" => "Integer loop", "samplesSeconds" => [0.2, 0.3, 0.4],
      "medianSeconds" => 0.3, "minimumSeconds" => 0.2, "comparable" => true
    }
  ]
}

html = PopBenchmark::Report.render(document)
[
  '<main class="dashboard">', 'id="workload"', 'id="metric"', 'id="ranking"',
  'id="distribution"', "Execution-only", "Fastest", "\\u003c/script>"
].each { |expected| BenchmarkAssertions.includes(html, expected) }
BenchmarkAssertions.includes(html, "infinite alternate")
BenchmarkAssertions.includes(html, "Math.log2")
BenchmarkAssertions.assert(!html.include?('class="ball"'), "legacy animated-ball report remains")

puts "benchmark tests: ok"
