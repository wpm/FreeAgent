# The desktop apps the release scripts build, sourced by them. An app's
# folder under apps/, its command line's crate and binary, and its tag
# prefix are all its product's name.
#
#   . product.sh && product_names <product>
#
# product_names sets title, the app's name ("Twenty Questions"), and stem,
# which begins its installers' stable names ("Twenty-Questions"), and fails
# for a product that is not a desktop app.
# shellcheck shell=bash

products="social-deduction twenty-questions"

product_names() { # <product>
  case $1 in
    social-deduction) title="Social Deduction" ;;
    twenty-questions) title="Twenty Questions" ;;
    *) echo "no desktop app named '$1': one of $products" >&2; return 1 ;;
  esac
  # shellcheck disable=SC2034 # used by the scripts that source this one
  stem=${title// /-}
}
