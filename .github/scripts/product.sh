# The desktop apps the release scripts build, sourced by them. An app's
# folder under apps/, its command line's crate and binary, and its tag
# prefix are all its product's name.
#
#   . product.sh && product_names <product>
#
# product_names sets title, the app's name ("Twenty Questions"); stem,
# which begins its installers' stable names ("Twenty-Questions"); and
# plays, what the app does, for its release notes. It fails for a product
# that is not a desktop app.
# shellcheck shell=bash

products="social-deduction twenty-questions"

# shellcheck disable=SC2034 # used by the scripts that source this one
product_names() { # <product>
  case $1 in
    social-deduction)
      title="Social Deduction"
      plays="plays one game of Werewolf or Avalon and shows it as a group chat beside a live board" ;;
    twenty-questions)
      title="Twenty Questions"
      plays="plays one game of Twenty Questions and shows it as a group chat, with the secret object and the guesses used on top" ;;
    *) echo "no desktop app named '$1': one of $products" >&2; return 1 ;;
  esac
  stem=${title// /-}
}
