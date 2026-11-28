#!/bin/bash
# Executed as a fixed script by pkexec. The udev rule is root-owned and records
# the delegate. Only the owner field changes; the previous owner is root.
set -euo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
export LC_ALL=C
umask 077
rule=/etc/udev/rules.d/60-thermal-berry.rules
disabled=/etc/udev/rules.d/.thermal-berry-revoking
[[ $EUID == 0 ]]

mkdir -p -m 0700 /run/thermal-berry-udev
exec 9>/run/thermal-berry-udev/lock
flock -x 9

delegate() {
    local file=$1 mode header
    local -a lines
    [[ -f $file && ! -L $file && $(stat -c '%u' "$file") == 0 ]] || return 1
    mode=$(stat -c '%a' "$file") || return 1
    (( (8#$mode & 022) == 0 )) || return 1
    mapfile -t lines < "$file"
    [[ ${#lines[@]} == 2 ]] || return 1
    header=${lines[0]}
    if [[ $header =~ ^#\ Thermal\ Berry\ sysfs\ delegate\ uid=([1-9][0-9]*)$ ]]; then
        local uid=${BASH_REMATCH[1]}
        [[ ${lines[1]} == "$(new_rule "$uid")" ]] || return 1
        printf '%s\n' "$uid"
    elif [[ $header =~ ^#\ Installed\ by\ thermal-berry:\ allows\ controlling\ fan\ boost\ without\ root\ \(user:\ ([a-zA-Z0-9_.-]+)\)$ ]]; then
        local user=${BASH_REMATCH[1]}
        local account
        # Match the second line as well before changing any attribute.
        [[ ${lines[1]} == "ACTION==\"add|change\", SUBSYSTEM==\"hwmon\", ATTR{name}==\"alienware_wmi\", RUN+=\"/bin/sh -c 'chown $user /sys%p/fan*_boost'\"" ]] || return 1
        account=$(getent passwd -- "$user") || return 1
        IFS=: read -r _ _ uid _ <<< "$account"
        [[ $uid =~ ^[1-9][0-9]*$ ]] || return 1
        printf '%s\n' "$uid"
    else
        return 1
    fi
}

new_rule() {
    printf 'ACTION=="add|change", SUBSYSTEM=="hwmon", ATTR{name}=="alienware_wmi", RUN+="/usr/bin/find /sys%%p -maxdepth 1 -regextype posix-extended -type f -uid 0 -regex .*/fan[0-9]+_boost -exec /usr/bin/chown -- +%s {} +"\n' "$1"
}

action=${1:-}
# Debian calls this same file as postrm with arguments such as upgrade or purge.
case $action in
    upgrade|abort-install|abort-upgrade|failed-upgrade|disappear) exit 0 ;;
esac
case $action in
    install)
        uid=${PKEXEC_UID:-}
        [[ $uid =~ ^[1-9][0-9]*$ ]] || { echo 'Cannot identify the requesting desktop user.' >&2; exit 1; }
        [[ ! -e $disabled && ! -L $disabled ]] || { echo 'Previous revocation must complete first.' >&2; exit 1; }
        if [[ -e $rule || -L $rule ]]; then
            installed=$(delegate "$rule") || { echo 'Existing udev rule is not managed by Thermal Berry.' >&2; exit 1; }
            [[ $installed == "$uid" ]] || { echo 'Another user already has a Thermal Berry grant.' >&2; exit 1; }
        fi
        staged=$(mktemp /etc/udev/rules.d/.thermal-berry.XXXXXX)
        trap 'rm -f -- "$staged"' EXIT
        printf '# Thermal Berry sysfs delegate uid=%s\n' "$uid" > "$staged"
        new_rule "$uid" >> "$staged"
        # The rule contains no shell expansion and changes only root-owned attributes.
        mv -T -- "$staged" "$rule"
        udevadm control --reload-rules
        udevadm trigger --subsystem-match=hwmon --action=change
        udevadm settle
        ;;
    remove|purge|remove-deb)
        if [[ -e $rule || -L $rule ]]; then
            [[ ! -e $disabled && ! -L $disabled ]] || exit 1
            delegate "$rule" >/dev/null || { echo 'Unrecognized udev rule.' >&2; exit 1; }
            mv -T -- "$rule" "$disabled"
        fi
        if [[ ! -e $disabled && ! -L $disabled ]]; then
            if [[ $action == remove-deb ]]; then
                flock -u 9
                exec /usr/bin/dpkg --purge thermal-berry
            fi
            exit 0
        fi
        uid=$(delegate "$disabled") || { echo 'Cannot verify the original udev grant.' >&2; exit 1; }
        udevadm control --reload-rules
        udevadm settle
        for directory in /sys/class/hwmon/hwmon*; do
            [[ -r $directory/name ]] && [[ $(<"$directory/name") == alienware_wmi ]] || continue
            for attribute in "$directory"/fan*_boost; do
                [[ -f $attribute && ! -L $attribute && ${attribute##*/} =~ ^fan[0-9]+_boost$ ]] || continue
                owner=$(stat -c '%u' "$attribute")
                case $owner in
                    "$uid") chown -h -- +0 "$attribute" ;;
                    0) ;;
                    *) echo 'A fan attribute has an unexpected owner. Revocation is incomplete.' >&2; exit 1 ;;
                esac
            done
        done
        rm -- "$disabled"
        if [[ $action == remove-deb ]]; then
            flock -u 9
            exec /usr/bin/dpkg --purge thermal-berry
        fi
        ;;
    *) exit 2 ;;
esac
