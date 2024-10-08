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

## Limiting blame history ##

Using `git-blame-filter` is useful in topic branches to edit commits not yet
in an integration branch (such as `main`, `master` or `dev`) by creating
`fixup!` commits. When doing so, blaming diff lines beyond that integration
branch is not helpful, as one usually does not want to create fixups versus
commits in the integration branch.

Blaming diff lines therefore can be limited to commits that are not in
the integration branch using the `--back-to`/`-b` option. This is done by
finding the best common ancestor of that branch and `HEAD` (the topic branch),
and then blame only up to that ancestor. Lines that do not come from commits
in the topic branch are annotated as `······`, increasing visibility of useful
commit-IDs significantly. As an extra, this may speed up blaming on large
projects.

## Configuration ##

Example `git-config`:

```
[alias]
    diff-blame = !git diff --color | blaming-diff-filter -- delta --color-only
[interactive]
    diffFilter = blaming-diff-filter -b master -- delta --color-only
```

To create fixup commits, run `git add -p`, stage hunks for a fixup, then do a
`git commit --fixup=<commitid>`, optionally followed by an interactive rebase.
