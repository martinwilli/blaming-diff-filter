# git blaming diffFilter #

`blaming-diff-filter` filters `git-diff` output and prefixes line information
with blamed commit-IDs. When used for `interactive.diffFilter`, this allows the
user during `git-add -p` to select hunks when creating fixup commits.

Inspiration from [git diff-blame](https://github.com/dmnd/git-diff-blame).

## Using wrapping filters ##

Optionally, an additional inner filter can be specified to pretty-print the
diff. The inner filter receives the raw diff, and `blaming-diff-filter` applies
commit-ID prefixes to the output of the inner filter.

This allows combining it with other fancy highlighting, such as from
[delta](https://github.com/dandavison/delta).

## Configuration ##

Example `git-config`:

```
[alias]
    diff-blame = !git diff --color | blaming-diff-filter -- delta --color-only
[interactive]
    diffFilter = blaming-diff-filter -- delta --color-only
```

To create fixup commits, run `git add -p`, stage hunks for a fixup, then do a
`git commit --fixup=<commitid>`, optionally followed by an interactive rebase.
