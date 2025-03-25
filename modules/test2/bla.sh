# THis is test 2
# The current module is {{DOD_CURRENT_MODULE}}
# {{ is_executable "test" }}
{{#if (is_executable "test")}}
# fuck 
{{/if}}
{{#if (is_executable "test")}}
# test is executable, wow!
{{ (find_executable "test") }}
{{/if}}
{{ to_upper_case (to_singular "Hello foo-bars") }}
