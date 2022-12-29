%define __spec_install_post %{nil}
%define __os_install_post %{_dbpath}/brp-compress
%define debug_package %{nil}

Summary: RPM tool
Name: rpm-tool
Version: %(cat VERSION)
Release: 1%{dist}
License: MailRu Private
Group: Applications/System
Source0: %{name}-%{version}.tar.gz
BuildRoot: %{_tmppath}/%{name}-%{version}-%{release}-root

AutoReqProv: no

BuildRequires: gcc
BuildRequires: autoconf
BuildRequires: automake
BuildRequires: libtool
BuildRequires: openssl-devel
BuildRequires: llvm-devel
BuildRequires: clang
BuildRequires: gzip
BuildRequires: cmake make

%description
%{summary}

Built by: %__hammer_user_name__ (%__hammer_user_login__)
From git commit: %__hammer_git_hash__ (%__hammer_git_ref__)

Build details: %__hammer_build_url__

%prep
%if 0%{?!__ci_build__:1}
%setup -q
%endif

%build
if [ -e VERSION ]; then
   sed -i -e "s/^package[.]version = .*/package.version = \"$(cat VERSION)\"/" Cargo.toml
fi
cargo build --release

%install
rm -rf %{buildroot}
%{__mkdir} -p %{buildroot}%{_bindir}
%{__mkdir} -p %{buildroot}%{_etcdir}

%{__install} -pD -m 755 target/release/rpm-tool %{buildroot}%{_bindir}/rpm-tool
%{__install} -pD -m 755 etc/rpm-tool.example.yaml %{buildroot}%{_etcdir}/rpm-tool.example.yaml

%clean
rm -rf %{buildroot}

%files
%defattr(-,root,root,-)
%{_bindir}/rpm-tool
%{_etcdir}/rpm-tool.example.yaml
