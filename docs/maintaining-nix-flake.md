# Maintaining the Nix Flake after new NBping release

When releasing a new version of NBping, follow these steps to update the Nix flake:

## 1. Update Version in flake.nix

Update the `version` field in `flake.nix` to match the new release version:

```nix
version = "0.6.2";  # Update this to the new version
```
## 2. Update flake.lock

Update the lock file to get the latest nixpkgs:

```bash
nix flake update
```

## 3. Update Cargo Hash (if dependencies changed)

If you've added, removed, or updated dependencies in `Cargo.toml`, you need to recalculate the `cargoHash`:

```bash
# Stage your changes
git add flake.nix Cargo.toml Cargo.lock

# Clear the hash temporarily
sed -i 's/cargoHash = ".*"/cargoHash = ""/' flake.nix

# Run build to get the correct hash
nix build --no-link 2>&1 | grep "got:"

# Copy the hash from the output and update flake.nix
# Example output: got:    sha256-XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX=
```

Update the `cargoHash` in `flake.nix` with the value from the output.

## 4. Test the Build

Verify everything works correctly:

```bash
# Test build
nix build --no-link

# Test version output
nix run . -- --version

# Verify flake structure
nix flake check

# Test development shell
nix develop -c cargo --version
```

## 5. Commit Changes

Commit all changes together:

```bash
git add flake.nix flake.lock
git commit -m "chore: update Nix flake for v0.6.2 release"
```
