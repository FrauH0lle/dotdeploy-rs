#!/usr/bin/env bash

echo "hey from test_foo.sh"
echo "We have been called with $@"
echo "Current module is $DOD_CURRENT_MODULE"

# fullname="USER INPUT"
read -p "Enter fullname: " fullname
echo "Name is : $fullname"
