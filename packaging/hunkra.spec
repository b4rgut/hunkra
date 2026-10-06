# A local binary distribution, assembled from an already checked locked Rust build.
# Product version comes from the workspace/release-plz; never bump it here.
%global debug_package %{nil}
%global _build_id_links none
%global source_date_epoch_from_changelog 0
%global use_source_date_epoch_as_buildtime 1
%global clamp_mtime_to_source_date_epoch 1
Name: hunkra
Version: %{hunkra_version}
Release: 1%{?dist}
Summary: Native Git changes and conflict editor
License: LicenseRef-Hunkra-Unselected
URL: https://github.com/b4rgut/hunkra
Source0: hunkra-payload.tar.gz
Requires: git-core
Requires: openssh-clients
Requires: font(dejavusans)
Requires: font(dejavusansmono)
Requires: libvulkan.so.1()(64bit)
Requires: libwayland-client.so.0()(64bit)
Requires: libfontconfig.so.1()(64bit)
Requires: libfreetype.so.6()(64bit)

%description
A native Rust/GPUI Git client with staged and unstaged diffs, partial staging,
commits, branch operations, independent conflict Result editing and history.
This artifact is prepared for local review; the product license is not selected.

%prep
%setup -q -n hunkra-payload

%build
# Compilation is completed by scripts/dev build-ui --release before packaging.

%install
mkdir -p %{buildroot}%{_bindir}
install -p -m 0755 hunkra %{buildroot}%{_bindir}/hunkra
mkdir -p %{buildroot}%{_datadir}/applications
install -p -m 0644 hunkra.desktop %{buildroot}%{_datadir}/applications/hunkra.desktop
mkdir -p %{buildroot}%{_datadir}/icons/hicolor/scalable/apps
install -p -m 0644 hunkra.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/hunkra.svg
mkdir -p %{buildroot}%{_datadir}/doc/hunkra
install -p -m 0644 README.md %{buildroot}%{_datadir}/doc/hunkra/README.md
mkdir -p %{buildroot}%{_datadir}/hunkra
install -p -m 0644 build-info.json %{buildroot}%{_datadir}/hunkra/build-info.json
mkdir -p %{buildroot}%{_datadir}/licenses/hunkra
install -p -m 0644 LICENSE-STATUS %{buildroot}%{_datadir}/licenses/hunkra/LICENSE-STATUS
cp -a third-party.json third-party %{buildroot}%{_datadir}/licenses/hunkra/

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/hunkra.desktop

%files
%{_bindir}/hunkra
%{_datadir}/applications/hunkra.desktop
%{_datadir}/icons/hicolor/scalable/apps/hunkra.svg
%doc %{_datadir}/doc/hunkra
%{_datadir}/hunkra
%license %{_datadir}/licenses/hunkra
