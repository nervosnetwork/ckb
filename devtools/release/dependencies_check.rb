#!/usr/bin/env ruby
# find unused dependencies in Cargo.toml
# this is a rough checker, please double check the result for special cases, e.g. features / macro / crate_name::fn()

require 'toml-rb'
require 'colorize'

def crates_in_rust_files(folder)
    Dir["#{folder}/**/*.rs"].inject([]) do |crates, file_name|
        File.readlines(file_name).grep(/use (\w*)(?:::|;)/) {|_| crates << $1}
        crates
    end.uniq
end

def crates_in_cargo_toml(folder)
    (TomlRB.load_file(File.join(folder, 'Cargo.toml'))['dependencies'] || {}).keys.map{|s| s.gsub(/-/, '_')}
end

def find_unused_dependencies(folder)
    puts "checking #{folder}/Cargo.toml"
    unused = crates_in_cargo_toml(folder) - crates_in_rust_files(folder)
    puts unused.empty? ? "OK".green : "Found #{unused}".red
end

folders = TomlRB.load_file(File.join(ARGV[0], 'Cargo.toml'))['workspace']['members'] + [ARGV[0]]
folders.each do |folder|
    find_unused_dependencies(folder)
end
