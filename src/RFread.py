import xml.etree.ElementTree as ET
import base64
import struct
import zlib
import sys
import os
from array import array

import RFcommonfn


def bin2float(node):
    d = base64.b64decode(node.findtext("{http://psi.hupo.org/ms/mzml}binary"))
    if node.find("*[@accession='MS:1000574']") is not None:
        d = zlib.decompress(d)
    fmt = (
        "<" + str(int(len(d) / 4)) + "f"
        if node.find("*[@accession='MS:1000523']") is None
        else "<" + str(int(len(d) / 8)) + "d"
    )
    return struct.unpack(fmt, d)


def store_scan(element):
    rt = element.find(".//*[@accession='MS:1000016']")
    rtinsec = float(rt.get("value"))
    if rt.get("unitName") == "minute":
        rtinsec *= 60
    mz = bin2float(element.find(".//*[@accession='MS:1000514'].."))
    inten = bin2float(element.find(".//*[@accession='MS:1000515'].."))
    return RFcommonfn.Spec(
        rtinsec,
        array("d", (m for m, i in zip(mz, inten) if i > 0)),
        array("d", (i for i in inten if i > 0)),
    )


def print_eic_ms(mzML_file):
    ms1_scans = []
    ms2_scans = []
    chr_scans = []

    for _, element in ET.iterparse(mzML_file):
        if element.tag == "{http://psi.hupo.org/ms/mzml}spectrum":
            if element.findtext(".//*{http://psi.hupo.org/ms/mzml}binary"):
                if element.find("*[@accession='MS:1000127']") is None:
                    print("error: profile mode!")
                    sys.exit()
                mslevel = element.find("*[@accession='MS:1000511']").attrib["value"]
                if mslevel == "1":
                    ms1_scans.append(store_scan(element))
                elif mslevel == "2":
                    ms2_scans.append(
                        (
                            float(
                                element.find(".//*[@accession='MS:1000744']").get(
                                    "value"
                                )
                            ),
                            store_scan(element),
                        )
                    )
                else:
                    sys.exit()
        elif element.tag == "{http://psi.hupo.org/ms/mzml}chromatogram":
            if element.findtext(".//*{http://psi.hupo.org/ms/mzml}binary"):
                title = element.attrib["id"]
                time_arr = bin2float(element.find(".//*[@accession='MS:1000595'].."))
                if (
                    element.find(".//*[@accession='MS:1000595']").attrib["unitName"]
                    == "minute"
                ):
                    time_arr = [x * 60 for x in time_arr]
                inten = bin2float(element.find(".//*[@accession='MS:1000515'].."))
                chr_scans.append((title, time_arr, inten))

    return ms1_scans, sorted(ms2_scans), chr_scans
