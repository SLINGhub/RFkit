import sys
import collections
import operator
import itertools
from bisect import bisect_left
import os
import glob
import concurrent.futures
import math
import time

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.backends.backend_pdf import PdfPages

import RFcommonfn
import RFread
import RFlib


Ent = RFcommonfn.Ent

param_set = {
    "mzML_files",
    "library",
    "ms1_ppm",
    "sample_info",
}
param_dict = RFcommonfn.read_param(param_set)
mzML_files = sorted(glob.glob(param_dict["mzML_files"]))
basename_l = [os.path.basename(x) for x in mzML_files]
ms1ppm = float(param_dict["ms1_ppm"]) / 1e6


def get_sample_info():
    sample_info_file = glob.glob(param_dict["sample_info"])[0]
    sample_type = dict()
    with open(sample_info_file) as sfile:
        next(sfile)
        for line in sfile:
            if line[0] != "#" and line.rstrip():
                lsp = line.rstrip("\n").split("\t")
                sample_type[lsp[0]] = lsp[1].upper()
    return sample_type


def cos_sim(list1, list2):
    if len(list1) != len(list2):
        print("adf")
        sys.exit()
    if sum(list1) <= 0 or sum(list2) <= 0:
        return 0
    return sum(math.sqrt(x * y) for x, y in zip(list1, list2)) / math.sqrt(
        sum(list1) * sum(list2)
    )


def aveMS1spec(mzML_file):
    basename0 = os.path.basename(mzML_file)
    print(basename0)
    ms1_scans, ms2_scans, _ = RFread.print_eic_ms(mzML_file)
    ms1_peaks = sorted(
        (mz, i, ii)
        for ii, ms1_ in enumerate(ms1_scans)
        for mz, i in zip(ms1_.mz_l, ms1_.I_l)
    )
    avespec = []
    while ms1_peaks:
        maxI = max(ms1_peaks, key=operator.itemgetter(1))[0]
        pos0 = bisect_left(ms1_peaks, (maxI - 0.005,))
        pos1 = bisect_left(ms1_peaks, (maxI + 0.005,))
        if len(set(ii for _, _, ii in ms1_peaks[pos0:pos1])) > len(ms1_scans) / 3:
            avespec.append(
                (maxI, sum(i for _, i, _ in ms1_peaks[pos0:pos1]) / len(ms1_scans))
            )
        del ms1_peaks[pos0:pos1]
    return sorted(avespec), ms2_scans, basename0


from multiprocessing import freeze_support

if __name__ == "__main__":
    start_time = time.time()
    print(len(mzML_files), "mzML files")
    freeze_support()
    sample_type = get_sample_info()
    with concurrent.futures.ProcessPoolExecutor(max_workers=9) as executor:
        avespec_l = list(executor.map(aveMS1spec, mzML_files))

    ms1_peaks = sorted((mz, i, ii) for ms1_, _, ii in avespec_l for mz, i in ms1_)
    conspec = []
    while ms1_peaks:
        maxI = max(ms1_peaks, key=operator.itemgetter(1))[0]
        pos0 = bisect_left(ms1_peaks, (maxI - 0.005,))
        pos1 = bisect_left(ms1_peaks, (maxI + 0.005,))
        if (
            len(
                set(
                    ii
                    for _, _, ii in ms1_peaks[pos0:pos1]
                    if sample_type[ii] != "BLANK"
                )
            )
            >= 5
        ):
            conspec.append(maxI)
        del ms1_peaks[pos0:pos1]
    lib_ent = RFlib.get_cpds()
    con_tab = dict()
    con_ms1 = dict()
    for ii, conmz in enumerate(sorted(conspec), 1):
        con_tab[(conmz, ii)] = dict()
        err_bd = RFcommonfn.bound_ppm(conmz * ms1ppm)
        pos_0 = bisect_left(lib_ent, (conmz - err_bd,))
        pos_1 = bisect_left(lib_ent, (conmz + err_bd,))
        ms1name = [
            x.replace("\n", "---")
            for _, x in sorted(
                set((abs(ent.Mmass - conmz), ent.name) for ent in lib_ent[pos_0:pos_1])
            )
        ]
        con_ms1[conmz] = ms1name

        for avespec, ms2_scans, basename0 in avespec_l:
            pos = bisect_left(ms2_scans, (conmz,))
            if pos > 0 and ms2_scans[pos][0] - conmz > conmz - ms2_scans[pos - 1][0]:
                pos -= 1
            expMS2 = ms2_scans[pos]
            top10 = sorted(expMS2[1].I_l, reverse=True)[min(len(expMS2[1].I_l) - 1, 9)]
            topN = [x for x in zip(expMS2[1].mz_l, expMS2[1].I_l) if x[1] >= top10]
            topmz = [x for x, _ in topN]
            topI = [x for _, x in topN]
            if abs(conmz - expMS2[0]) > 0.51:
                print(
                    "{} {} {} {}".format(
                        abs(conmz - expMS2[0]), expMS2[0], conmz, basename0
                    )
                )

            score_ent = []
            for ent in lib_ent[pos_0:pos_1]:
                ms2_I = []
                ent_I = []
                xfrag = set()
                hpeak = 0
                mpeak = 0
                fent = sorted(
                    [
                        (y, x)
                        for x, y in zip(ent.mz, ent.I)
                        if (ent.charge * conmz - x) > 3.3
                    ],
                    reverse=True,
                )[:10]
                for f_I, f_mz in fent:
                    err_bd = 0.01
                    pos0 = bisect_left(topmz, f_mz - err_bd)
                    pos1 = bisect_left(topmz, f_mz + err_bd, lo=pos0)
                    ent_I.append(f_I)

                    if pos0 != pos1:
                        ms2_I.append(max(topI[pos0:pos1]))
                        for i in range(pos0, pos1):
                            xfrag.add(i)
                        if f_I == fent[0][0]:
                            hpeak = 1
                        mpeak += 1
                    else:
                        ms2_I.append(0)

                if hpeak:
                    for nn, (f_mz, f_I) in enumerate(zip(topmz, topI)):
                        if nn not in xfrag and (ent.charge * conmz - f_mz) > 3.3:
                            ms2_I.append(f_I)
                            ent_I.append(0)
                    cs = cos_sim(ent_I, ms2_I)
                    score_ent.append((cs, ent, mpeak))
            pos0 = bisect_left(avespec, (conmz - 0.005,))
            pos1 = bisect_left(avespec, (conmz + 0.005,))
            aveI = sum(x for _, x in avespec[pos0:pos1])
            ave_mz = (
                sum(mz * i for mz, i in avespec[pos0:pos1]) / aveI if aveI > 0 else None
            )
            if score_ent:
                score_ent = max(score_ent)
                sc = score_ent[0]
                mpeak = score_ent[2]
                ent = score_ent[1]
                exp_mz = expMS2[1].mz_l
                exp_I = expMS2[1].I_l
            else:
                sc = None
                mpeak = None
                ent = Ent(
                    conmz, "m/z={:.4f}".format(conmz), tuple(), tuple(), "", 1, None, ""
                )
                exp_mz = exp_I = tuple()
            con_tab[(conmz, ii)][basename0] = (
                aveI,
                sc,
                ent,
                exp_mz,
                exp_I,
                ave_mz,
                mpeak,
            )

    for basename0 in basename_l:
        open("ann_{}.txt".format(basename0), "w")

    frago = open("quant_frag.txt", "w")
    frago.write(
        "group\tID\talt_IDs\tMS1\tmz\tadduct\tID\tcount\tfrag_m/z\t"
        + "\t".join(basename_l)
        + "\t"
        + "\t".join("score_" + x for x in basename_l)
        + "\t"
        + "\t".join("mass_error_" + x for x in basename_l)
        + "\n"
    )

    for x, y in sorted(con_tab.items(), key=lambda x: x[0][0]):
        c = collections.Counter(yy[2] for yy in y.values())
        ent, cn = c.most_common(1)[0]
        identified = (
            "" if all(yy[2].name.startswith("m/z=") for yy in y.values()) else "*"
        )
        id_with_count = "{} ({})".format(ent.name.replace("\n", "---"), cn)
        alt_id_with_count = " --- ".join(
            "{} ({})".format(ent.name.replace("\n", "---"), cn)
            for ent, cn in c.most_common()[1:]
        )
        count_pos = sum(1 for qs in y.values() if qs[0] > 0)
        frago.write(
            "{}\t{}\t{}\t{}\t{:.4f}\t{}\t{}\t{}\t{}".format(
                x[1],
                id_with_count,
                alt_id_with_count,
                " --- ".join(con_ms1[x[0]]),
                x[0],
                ent.adduct,
                identified,
                count_pos,
                "precursor",
            )
        )

        for basename0 in basename_l:
            qs = y[basename0]
            frago.write("\t{:.1f}".format(qs[0]))
        for basename0 in basename_l:
            qs = y[basename0]
            frago.write("\t{:.2f}".format(qs[1], qs[6]) if qs and qs[1] else "\t")
        for basename0 in basename_l:
            qs = y[basename0]
            frago.write(
                "\t{:.3f}".format(ent.Mmass - qs[5]) if qs[1] and qs[5] else "\t"
            )

        frago.write("\n")

        if ent.mz:
            mzML_f = dict()
            for basename0 in basename_l:
                qs = y[basename0]
                mzML_f[basename0] = dict()
                for f_mz in ent.mz:
                    pos0 = bisect_left(qs[3], f_mz - 0.01)
                    pos1 = bisect_left(qs[3], f_mz + 0.01)
                    mzML_f[basename0][f_mz] = (
                        max(qs[4][pos0:pos1]) if pos0 < pos1 else 0
                    )
                maxf = max(mzML_f[basename0].values())
                if maxf > 0:
                    for f_mz in ent.mz:
                        mzML_f[basename0][f_mz] /= maxf

        for f_mz, _ in sorted(
            zip(ent.mz, ent.I), key=operator.itemgetter(1), reverse=True
        ):
            frago.write(
                "{}\t{}\t{}\t{}\t{:.4f}\t{}\t{}\t{}\t{}".format(
                    x[1],
                    id_with_count,
                    alt_id_with_count,
                    "",
                    x[0],
                    ent.adduct,
                    identified,
                    sum(1 for basename0 in basename_l if mzML_f[basename0][f_mz] > 0),
                    f_mz,
                )
            )
            for basename0 in basename_l:
                frago.write("\t{:.2f}".format(mzML_f[basename0][f_mz]))
            frago.write("\n")

        for basename0 in basename_l:
            qs = y[basename0]
            if qs[1]:
                with open("ann_{}.txt".format(os.path.basename(basename0)), "a") as ann:
                    ann.write("NAME:\n")
                    ann.write("{}\n".format(qs[2].name))
                    ann.write("ADDUCT: {}\n".format(qs[2].adduct))
                    ann.write("TARGET_M/Z: {:.6f}\n".format(x[0]))
                    ann.write("DOT_PRODUCT: {:.3f}\n".format(qs[1]))
                    ann.write("EXPERIMENTAL_SPECTRUM:\n")
                    for mz, i in sorted(
                        zip(qs[3], qs[4]), key=operator.itemgetter(1), reverse=True
                    ):
                        ann.write("{:.6f} {:.2f}\n".format(mz, i))
                    ann.write("LIBRARY_SPECTRUM:\n")
                    for mz, i in zip(qs[2].mz, qs[2].I):
                        ann.write("{:.6f} {:.2f}\n".format(mz, i))
                    ann.write("\n")

    with PdfPages("aveMS1spec.pdf") as pdf0:
        for avespec, _, basename0 in avespec_l:
            plt.figure(figsize=(9, 4))
            ax = plt.subplot(1, 1, 1)
            ax.set_title("{}  ave. MS1".format(basename0[:-5]))
            exp_ = ax.vlines(
                x=[mz for mz, _ in avespec],
                ymin=0,
                ymax=[i for _, i in avespec],
                color="black",
                lw=0.5,
            )
            ax.set_xlabel("m/z")  # ,fontsize=22)
            ax.set_ylabel("intensity")  # ,fontsize=22)
            pdf0.savefig()
            plt.close()

    print("Run time = {:.1f} mins".format(((time.time() - start_time) / 60)))
